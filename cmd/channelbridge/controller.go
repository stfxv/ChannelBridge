// SPDX-License-Identifier: GPL-3.0-only
package main

import (
	"channelbridge/internal/native"
	"channelbridge/internal/settings"
	"encoding/json"
	"errors"
	"fmt"
	"time"
)

type captureState string

const (
	stateIdle       captureState = "idle"
	stateConnecting captureState = "connecting"
	statePreview    captureState = "preview"
	stateSending    captureState = "sending"
	stateRecovering captureState = "recovering"
	stateBlocked    captureState = "blocked"
)

type model struct {
	State      captureState    `json:"state"`
	PeakLeft   float32         `json:"peak_left"`
	PeakRight  float32         `json:"peak_right"`
	Overview   bool            `json:"overview"`
	AllLevels  []float32       `json:"all_levels"`
	Config     settings.Config `json:"config"`
	Devices    []native.Device `json:"devices"`
	Driver     bool            `json:"driver"`
	Running    bool            `json:"running"`
	Monitoring bool            `json:"monitoring"`
	Wanted     bool            `json:"wanted"`
	Status     string          `json:"status"`
	Left       float32         `json:"level_left"`
	Right      float32         `json:"level_right"`
	Format     string          `json:"input_format"`
}

// audioBackend keeps recovery policy testable without recording or installing a driver.
type audioBackend interface {
	Devices() ([]native.Device, error)
	DriverAvailable() bool
	DeviceRevision() uint64
	SetMuted(bool)
	SetOverview(bool)
	AllLevels() []float32
	Start(string, int, int, int, bool) error
	Preview(string, int, int) error
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
func (nativeAudio) Preview(uid string, l, r int) error         { return native.Preview(uid, l, r) }
func (nativeAudio) Stop()                                      { native.Stop() }
func (nativeAudio) Healthy() bool                              { return native.Healthy() }
func (nativeAudio) Levels() (float32, float32, uint64, uint64) { return native.Levels() }

func (nativeAudio) DeviceRevision() uint64 { return native.DeviceRevision() }
func (nativeAudio) SetMuted(v bool)        { native.SetMuted(v) }
func (nativeAudio) SetOverview(v bool)     { native.SetOverview(v) }
func (nativeAudio) AllLevels() []float32   { return native.AllLevels() }

type controller struct {
	audio               audioBackend
	now                 func() time.Time
	present             func([]byte)
	view                model
	path                string
	nextAttempt         time.Time
	retryDelay          time.Duration
	stableSignature     string
	stableSince         time.Time
	lastCallbacks       uint64
	lastAudio           time.Time
	ticks               int
	permissionAllowed   bool
	devicesReady        bool
	deviceRevision      uint64
	blockedSignature    string
	peakLeft, peakRight heldPeak
}

func (c *controller) refresh() {
	devices, err := c.audio.Devices()
	if err != nil {
		c.devicesReady = false
		c.deviceRevision = c.audio.DeviceRevision()
		c.view.Status = "入力デバイスの取得に失敗しました: " + err.Error()
		return
	}
	c.devicesReady = true
	c.deviceRevision = c.audio.DeviceRevision()
	c.view.Devices = devices
	c.view.Driver = c.audio.DriverAvailable()
}
func (c *controller) selected() (native.Device, bool) {
	if !c.devicesReady {
		return native.Device{}, false
	}
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
func (c *controller) setState(state captureState) {
	c.view.State = state
	c.view.Running = state == stateSending
	c.view.Monitoring = state == statePreview
}
func (c *controller) stop() {
	c.audio.Stop()
	c.setState(stateIdle)
	c.view.Left, c.view.Right = meterFloorDB, meterFloorDB
	c.view.PeakLeft, c.view.PeakRight = meterFloorDB, meterFloorDB
	c.peakLeft, c.peakRight = heldPeak{}, heldPeak{}
	c.view.AllLevels = nil
}
func (c *controller) start() { c.startCapture(false) }
func (c *controller) preview() {
	if !c.view.Wanted && !c.view.Monitoring && c.permissionAllowed && c.view.Config.DeviceUID != "" {
		c.startCapture(true)
	}
}
func (c *controller) resetRetry() {
	c.nextAttempt = time.Time{}
	c.retryDelay = time.Second
	c.stableSignature = ""
	c.blockedSignature = ""
}
func (c *controller) startCapture(preview bool) {
	d, ok := c.selected()
	if !ok {
		if c.devicesReady {
			c.view.Status = "選択した入力の再接続を待っています"
		}
		c.setState(stateRecovering)
		return
	}
	if !preview && !c.view.Driver {
		c.view.Status = "仮想マイクをセットアップしてください。インストール後はMacを再起動します"
		return
	}
	if err := c.view.Config.Validate(d.Channels); err != nil {
		c.view.Status = err.Error()
		c.view.Wanted = false
		c.setState(stateBlocked)
		c.blockedSignature = c.captureSignature(preview)
		return
	}
	mode := map[string]int{"mono": 0, "stereo": 1, "mix": 2}[c.view.Config.Mode]
	right := c.view.Config.Right - 1
	if mode == 0 {
		right = c.view.Config.Left - 1
	}
	// Changing capture mode always releases the previous IOProc first.
	c.stop()
	c.setState(stateConnecting)
	c.view.Status = "入力の接続を準備しています…"
	c.publish()
	var err error
	if preview {
		err = c.audio.Preview(d.UID, c.view.Config.Left-1, right)
	} else {
		err = c.audio.Start(d.UID, c.view.Config.Left-1, right, mode, c.view.Config.Muted)
	}
	if err != nil {
		var audioError *native.AudioError
		if errors.As(err, &audioError) && !audioError.Retryable {
			c.setState(stateBlocked)
			c.blockedSignature = c.captureSignature(preview)
			c.view.Status = err.Error() + "。入力形式・設定を確認してください"
			return
		}
		c.setState(stateRecovering)
		c.view.Status = "再接続を待機: " + err.Error()
		c.nextAttempt = c.now().Add(c.retryDelay)
		c.retryDelay *= 2
		if c.retryDelay > 8*time.Second {
			c.retryDelay = 8 * time.Second
		}
		return
	}
	if preview {
		c.setState(statePreview)
	} else {
		c.setState(stateSending)
	}
	c.view.Left, c.view.Right = meterFloorDB, meterFloorDB
	c.lastCallbacks = 0
	c.lastAudio = c.now()
	c.retryDelay = time.Second
	if preview {
		c.view.Status = "入力レベルを確認中 · 仮想マイクへの送出は停止中"
		return
	}
	c.view.Status = fmt.Sprintf("接続中 · %s · %.1f kHz → ChannelBridge Mic", d.Name, d.Rate/1000)
}

// The independent 250 ms worker timer supervises recovery. It coalesces rate/device
// changes, waits for stable hardware, and retries with bounded backoff. User Stop
// cancels send intent while input-only metering continues. A disconnect never
// discards the selected device UID. Preview uses the same recovery supervisor.
func (c *controller) tick() {
	c.ticks++
	if c.view.Running || c.view.Monitoring {
		l, r, callbacks, faults := c.audio.Levels()
		c.view.Left, c.view.Right = peakDBFS(l), peakDBFS(r)
		c.view.PeakLeft = c.peakLeft.update(c.view.Left, c.now())
		c.view.PeakRight = c.peakRight.update(c.view.Right, c.now())
		if c.view.Overview {
			c.view.AllLevels = c.audio.AllLevels()
			for i, peak := range c.view.AllLevels {
				c.view.AllLevels[i] = peakDBFS(peak)
			}
		}
		if callbacks != c.lastCallbacks {
			c.lastCallbacks = callbacks
			c.lastAudio = c.now()
		}
		if !c.audio.Healthy() || faults > 32 || c.now().Sub(c.lastAudio) > 2*time.Second {
			c.stop()
			c.view.Status = "入力の変更を検知しました。入力を再接続しています…"
			c.setState(stateRecovering)
			c.stableSignature = ""
			c.refresh()
			c.nextAttempt = c.now().Add(time.Second)
		}
	}
	if c.ticks%20 == 0 || c.audio.DeviceRevision() != c.deviceRevision {
		c.refresh()
	}
	preview := !c.view.Wanted && c.permissionAllowed && c.view.Config.DeviceUID != ""
	if !c.view.Running && !c.view.Monitoring && (c.view.Wanted || preview) {
		_, ok := c.selected()
		signature := c.captureSignature(preview)
		if signature != c.stableSignature {
			c.stableSignature = signature
			c.stableSince = c.now()
		}
		if !ok && c.devicesReady && c.view.Config.DeviceUID != "" {
			c.view.Status = "選択した入力の再接続を待っています"
		}
		if ok && signature != c.blockedSignature && (preview || c.view.Driver) && c.now().Sub(c.stableSince) >= time.Second && !c.now().Before(c.nextAttempt) {
			c.startCapture(preview)
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
		c.resetRetry()
		c.refresh()
	case "permission_granted":
		c.permissionAllowed = true
		c.preview()
	case "start":
		c.permissionAllowed = true // AppKit has checked/requested authorization.
		c.resetRetry()
		c.view.Wanted = true
		c.stop()
		c.refresh()
		c.start()
	case "stop":
		c.view.Wanted = false
		c.stop()
		c.resetRetry()
		c.view.Status = "送出は停止中"
		c.preview()
	case "config":
		var next settings.Config
		if err := json.Unmarshal([]byte(payload), &next); err != nil {
			c.view.Status = err.Error()
			break
		}
		// Validate before saving/applying. A missing saved channel remains a valid
		// selection to preserve, but cannot capture until it exists again.
		if err := next.ValidateSelection(); err != nil {
			c.view.Status = err.Error()
			break
		}
		if err := settings.Save(c.path, next); err != nil {
			c.view.Status = "設定の保存に失敗しました: " + err.Error()
			break // Keep both the live route and previous in-memory configuration.
		}
		old := c.view.Config
		c.view.Config = next
		if old.DeviceUID == next.DeviceUID && old.Left == next.Left && old.Right == next.Right && old.Mode == next.Mode {
			c.audio.SetMuted(next.Muted)
			break // Changing mute never reconstructs the audio route.
		}
		c.stop()
		c.resetRetry()
		c.refresh()
		if c.view.Wanted {
			c.start()
		} else {
			c.view.Status = "設定を保存しました"
			c.preview()
		}
	case "overview":
		var request struct {
			Enabled bool `json:"enabled"`
		}
		if json.Unmarshal([]byte(payload), &request) == nil {
			c.view.Overview = request.Enabled
			c.view.AllLevels = nil
			c.audio.SetOverview(request.Enabled)
		}
	case "permission_denied":
		c.permissionAllowed = false
		c.view.Wanted = false
		c.stop()
		c.view.Status = "システム設定 → プライバシーとセキュリティ → マイクで ChannelBridge を許可してください"
	case "permission_pending":
		c.permissionAllowed = false
		c.view.Wanted = false
		c.stop()
		c.setState(stateBlocked)
		c.view.Status = "macOSのマイク使用許可を待っています。許可ダイアログを確認してください"
	case "quit":
		c.view.Wanted = false
		c.stop()
		return
	}
	c.publish()
}

func (c *controller) captureSignature(preview bool) string {
	d, ok := c.selected()
	return fmt.Sprintf("%t/%t/%t/%s/%d/%g/%v", preview, ok, c.view.Driver, d.UID, d.Channels, d.Rate, d.Formats)
}
