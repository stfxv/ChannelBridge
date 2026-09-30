// SPDX-License-Identifier: GPL-3.0-only
package main

import (
	"encoding/json"
	"fmt"
	"os"
	"runtime"
	"time"

	"channelbridge/internal/native"
	"channelbridge/internal/settings"
)

type model struct {
	Config  settings.Config `json:"config"`
	Devices []native.Device `json:"devices"`
	Driver  bool            `json:"driver"`
	Running bool            `json:"running"`
	Wanted  bool            `json:"wanted"`
	Status  string          `json:"status"`
	Left    float32         `json:"level_left"`
	Right   float32         `json:"level_right"`
	Format  string          `json:"input_format"`
}

// audioBackend keeps recovery policy testable without recording or installing a driver.
type audioBackend interface {
	Devices() ([]native.Device, error)
	DriverAvailable() bool
	Start(string, int, int, int, bool) error
	Stop()
	Healthy() bool
	Levels() (float32, float32, uint64, uint64)
}
type nativeAudio struct{}

func (nativeAudio) Devices() ([]native.Device, error) { return native.Devices() }
func (nativeAudio) DriverAvailable() bool             { return native.DriverAvailable() }
func (nativeAudio) Start(uid string, l, r, mode int, muted bool) error {
	return native.Start(uid, l, r, mode, muted)
}
func (nativeAudio) Stop()                                      { native.Stop() }
func (nativeAudio) Healthy() bool                              { return native.Healthy() }
func (nativeAudio) Levels() (float32, float32, uint64, uint64) { return native.Levels() }

type controller struct {
	audio           audioBackend
	now             func() time.Time
	present         func([]byte)
	view            model
	path            string
	nextAttempt     time.Time
	retryDelay      time.Duration
	stableSignature string
	stableSince     time.Time
	lastCallbacks   uint64
	lastAudio       time.Time
	ticks           int
}

func (c *controller) refresh() {
	devices, err := c.audio.Devices()
	if err != nil {
		c.view.Status = "入力デバイスの取得に失敗しました: " + err.Error()
		return
	}
	c.view.Devices = devices
	c.view.Driver = c.audio.DriverAvailable()
}
func (c *controller) selected() (native.Device, bool) {
	for _, d := range c.view.Devices {
		if d.UID == c.view.Config.DeviceUID {
			return d, true
		}
	}
	return native.Device{}, false
}
func (c *controller) publish() {
	c.view.Format = "入力フォーマット: 未接続"
	if d, ok := c.selected(); ok {
		c.view.Format = inputFormat(d, c.view.Config)
	}
	b, _ := json.Marshal(c.view)
	if c.present != nil {
		c.present(b)
	}
}
func (c *controller) stop() {
	c.audio.Stop()
	c.view.Running = false
	c.view.Left, c.view.Right = meterFloorDB, meterFloorDB
}
func (c *controller) start() {
	d, ok := c.selected()
	if !ok {
		c.view.Status = "選択した入力の再接続を待っています"
		return
	}
	if !c.view.Driver {
		c.view.Status = "仮想マイクをセットアップしてください。インストール後はMacを再起動します"
		return
	}
	if err := c.view.Config.Validate(d.Channels); err != nil {
		c.view.Status = err.Error()
		c.view.Wanted = false
		return
	}
	mode := map[string]int{"mono": 0, "stereo": 1, "mix": 2}[c.view.Config.Mode]
	right := c.view.Config.Right - 1
	if mode == 0 {
		right = c.view.Config.Left - 1
	}
	err := c.audio.Start(d.UID, c.view.Config.Left-1, right, mode,
		c.view.Config.Muted)
	if err != nil {
		c.view.Status = "再接続を待機: " + err.Error()
		c.nextAttempt = c.now().Add(c.retryDelay)
		c.retryDelay *= 2
		if c.retryDelay > 8*time.Second {
			c.retryDelay = 8 * time.Second
		}
		return
	}
	c.view.Running = true
	c.view.Left, c.view.Right = meterFloorDB, meterFloorDB
	c.lastCallbacks = 0
	c.lastAudio = c.now()
	c.retryDelay = time.Second
	c.view.Status = fmt.Sprintf("接続中 · %s · %.1f kHz → ChannelBridge Mic", d.Name, d.Rate/1000)
}

// The 250 ms UI timer is also the recovery supervisor. It coalesces rate/device
// changes, waits for stable hardware, and retries with bounded backoff. User Stop
// cancels this intent; a disconnect never discards the selected device UID.
func (c *controller) tick() {
	c.ticks++
	if c.view.Running {
		l, r, callbacks, faults := c.audio.Levels()
		c.view.Left, c.view.Right = peakDBFS(l), peakDBFS(r)
		if callbacks != c.lastCallbacks {
			c.lastCallbacks = callbacks
			c.lastAudio = c.now()
		}
		if !c.audio.Healthy() || faults > 32 || c.now().Sub(c.lastAudio) > 2*time.Second {
			c.stop()
			c.view.Status = "入力の変更を検知しました。音声経路を再接続しています…"
			c.stableSignature = ""
			c.nextAttempt = c.now().Add(time.Second)
		}
	}
	if c.ticks%2 == 0 {
		c.refresh()
	}
	if c.view.Wanted && !c.view.Running {
		d, ok := c.selected()
		signature := fmt.Sprintf("%t/%t/%s/%d/%f", ok, c.view.Driver, d.UID, d.Channels, d.Rate)
		if signature != c.stableSignature {
			c.stableSignature = signature
			c.stableSince = c.now()
		}
		if ok && c.view.Driver && c.now().Sub(c.stableSince) >= time.Second && c.now().After(c.nextAttempt) {
			c.start()
		}
	}
	c.publish()
}
func (c *controller) action(action, payload string) {
	switch action {
	case "tick":
		c.tick()
		return
	case "refresh":
		c.refresh()
	case "start":
		c.view.Wanted = true
		c.refresh()
		c.start()
	case "stop":
		c.view.Wanted = false
		c.stop()
		c.view.Status = "停止中"
	case "config":
		var next settings.Config
		if err := json.Unmarshal([]byte(payload), &next); err != nil {
			c.view.Status = err.Error()
			break
		}
		c.stop()
		c.view.Config = next
		if err := settings.Save(c.path, next); err != nil {
			c.view.Status = "設定の保存に失敗しました: " + err.Error()
			break
		}
		if c.view.Wanted {
			c.refresh()
			c.start()
		} else {
			c.view.Status = "設定を保存しました"
		}
	case "permission_denied":
		c.view.Wanted = false
		c.stop()
		c.view.Status = "システム設定 → プライバシーとセキュリティ → マイクで ChannelBridge を許可してください"
	case "quit":
		c.view.Wanted = false
		c.stop()
		return
	}
	c.publish()
}
func main() {
	runtime.LockOSThread() // AppKit runs on the main OS thread.
	if len(os.Args) > 1 && os.Args[1] == "--devices" {
		d, err := native.Devices()
		if err != nil {
			fmt.Fprintln(os.Stderr, err)
			os.Exit(1)
		}
		b, _ := json.MarshalIndent(d, "", "  ")
		fmt.Println(string(b))
		return
	}
	path, err := settings.Path()
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
	config, err := settings.Load(path)
	c := &controller{audio: nativeAudio{}, now: time.Now, present: native.Present, path: path, retryDelay: time.Second, view: model{Config: config, Left: meterFloorDB, Right: meterFloorDB, Status: "入力を選び「開始」を押してください"}}
	if err != nil {
		c.view.Config = settings.Default()
		c.view.Status = "保存設定を読み込めませんでした。入力を再設定してください"
	}
	c.refresh()
	native.OnAction = c.action
	b, _ := json.Marshal(c.view)
	native.Run(b)
	native.Stop()
}
