package main

import (
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"channelbridge/internal/native"
	"channelbridge/internal/settings"
)

func configPayload(t *testing.T, config settings.Config) string {
	t.Helper()
	b, err := json.Marshal(config)
	if err != nil {
		t.Fatal(err)
	}
	return string(b)
}

func TestMuteDoesNotRestartCapture(t *testing.T) {
	c, a, _ := fixture(t)
	c.path = filepath.Join(t.TempDir(), "settings.json")
	starts, stops := a.starts, a.stops
	next := c.view.Config
	next.Muted = true
	c.action("config", configPayload(t, next))
	if a.starts != starts || a.stops != stops || !a.muted || !c.view.Running {
		t.Fatal("mute must only change the live atomic flag, keeping the route")
	}
	c.action("stop", "{}")
	previews, stops := a.previews, a.stops
	next.Muted = false
	c.action("config", configPayload(t, next))
	if a.previews != previews || a.stops != stops || !c.view.Monitoring {
		t.Fatal("mute must not interrupt input-only monitoring either")
	}
}

func TestSaveFailurePreservesLiveRouteAndSettings(t *testing.T) {
	c, a, advance := fixture(t)
	blocker := filepath.Join(t.TempDir(), "not-a-directory")
	if err := os.WriteFile(blocker, []byte("x"), 0600); err != nil {
		t.Fatal(err)
	}
	c.path = filepath.Join(blocker, "settings.json")
	old := c.view.Config
	next := old
	next.Left, next.Muted = 16, true
	starts, stops := a.starts, a.stops
	c.action("config", configPayload(t, next))
	advance(40)
	if c.view.Config != old || a.starts != starts || a.stops != stops || !c.view.Running {
		t.Fatal("failed save must not change or restart the live route")
	}
	if !strings.Contains(c.view.Status, "保存に失敗") {
		t.Fatal("save failure must remain visible")
	}
	// Reject invalid input before even writing a settings file.
	c.path = filepath.Join(t.TempDir(), "settings.json")
	next.Left = 0
	c.action("config", configPayload(t, next))
	if _, err := os.Stat(c.path); !os.IsNotExist(err) {
		t.Fatal("invalid selection must not be saved")
	}
}

func TestUnsupportedFormatWaitsForChangeOrManualRetry(t *testing.T) {
	c, a, advance := fixture(t)
	a.permanent, a.healthy = true, false
	advance(12)
	starts := a.starts
	advance(120)
	if a.starts != starts || c.view.State != stateBlocked {
		t.Fatal("unsupported format must not retry forever")
	}
	a.permanent = false
	c.action("refresh", "{}")
	advance(12)
	if !c.view.Running {
		t.Fatal("manual refresh must allow another attempt")
	}
}

func TestInventoryCacheAndNotifications(t *testing.T) {
	c, a, advance := fixture(t)
	reads := a.deviceReads
	advance(19)
	if a.deviceReads != reads {
		t.Fatal("stable inventory must not be enumerated every half second")
	}
	advance(1)
	if a.deviceReads != reads+1 {
		t.Fatal("five-second fallback refresh is required")
	}
	a.device.Channels = 64 // Revision changes independently of healthy route.
	advance(1)
	if c.view.Devices[0].Channels != 64 {
		t.Fatal("inventory notification must refresh promptly")
	}
}

func TestOverviewOnlyWhenRequestedAndClearedOnStop(t *testing.T) {
	c, _, advance := fixture(t)
	advance(1)
	if c.view.AllLevels != nil {
		t.Fatal("hidden overview must do no metering work")
	}
	c.action("overview", `{"enabled":true}`)
	advance(1)
	if len(c.view.AllLevels) != 2 || c.view.AllLevels[0] != -20 {
		t.Fatal("overview must report individual dBFS")
	}
	c.action("permission_denied", "{}")
	if c.view.AllLevels != nil || c.view.PeakLeft != meterFloorDB {
		t.Fatal("stopped capture cannot display stale levels")
	}
}

type inventoryFailure struct{ *fakeAudio }

func (a inventoryFailure) Devices() ([]native.Device, error) {
	return nil, errors.New("HAL unavailable")
}
func TestInventoryFailureDoesNotUseStaleDevice(t *testing.T) {
	c, a, _ := fixture(t)
	c.stop()
	c.audio = inventoryFailure{a}
	c.refresh()
	starts := a.starts
	c.start()
	if a.starts != starts || !strings.Contains(c.view.Status, "HAL unavailable") {
		t.Fatal("enumeration failure must not look like unplug or reuse stale metadata")
	}
}

func TestPeakHoldExpiresAndResets(t *testing.T) {
	now := time.Unix(0, 0)
	var peak heldPeak
	if peak.update(1, now) != 1 || peak.update(-20, now.Add(2*time.Second)) != 1 {
		t.Fatal("brief clipping must stay visible for three seconds")
	}
	if peak.update(-20, now.Add(3*time.Second)) != -20 {
		t.Fatal("peak must expire using elapsed time")
	}
	if peak.update(-6, now.Add(4*time.Second)) != -6 {
		t.Fatal("new larger peak must replace the held one")
	}
}

type blockingAudio struct {
	*fakeAudio
	started, release chan struct{}
}

func (a *blockingAudio) Start(uid string, l, r, mode int, muted bool) error {
	close(a.started)
	<-a.release
	return a.fakeAudio.Start(uid, l, r, mode, muted)
}
func TestControlWorkerAcceptsStopDuringSlowStart(t *testing.T) {
	a := &blockingAudio{fakeAudio: &fakeAudio{connected: true, healthy: true, device: native.Device{UID: "test", Channels: 32, Rate: 48000}}, started: make(chan struct{}), release: make(chan struct{})}
	config := settings.Default()
	config.DeviceUID = "test"
	snapshots := make(chan model, 32)
	c := &controller{audio: a, now: time.Now, retryDelay: time.Second, view: model{Config: config}, present: func(b []byte) {
		var snapshot model
		if err := json.Unmarshal(b, &snapshot); err != nil {
			panic(err)
		}
		snapshots <- snapshot
	}}
	runner := newControlRunner(c)
	go runner.run()
	runner.send("start", "{}")
	select {
	case <-a.started:
	case <-time.After(3 * time.Second):
		t.Fatal("start not dispatched")
	}
	accepted := make(chan struct{})
	go func() { runner.send("stop", "{}"); close(accepted) }()
	select {
	case <-accepted:
	case <-time.After(time.Second):
		t.Fatal("UI action blocked behind HAL start")
	}
	close(a.release)
	timeout := time.After(3 * time.Second)
	for {
		select {
		case snapshot := <-snapshots:
			if snapshot.Monitoring && !snapshot.Wanted {
				runner.send("quit", "{}")
				if a.active != "" {
					t.Fatal("shutdown must release capture before returning")
				}
				return
			}
		case <-timeout:
			t.Fatal("queued Stop must finish in input preview without resuming send")
		}
	}
}
