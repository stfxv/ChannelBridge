package main

import (
	"errors"
	"math"
	"testing"
	"time"

	"channelbridge/internal/native"
	"channelbridge/internal/settings"
)

type fakeAudio struct {
	device                   native.Device
	connected, healthy, fail bool
	starts, stops, previews  int
	driverMissing            bool
	active                   string
	left, right              int
	callbacks                uint64
	rates                    []float64
	muted                    bool
	muteChanges, deviceReads int
	permanent                bool
}

func (a *fakeAudio) Devices() ([]native.Device, error) {
	a.deviceReads++
	if !a.connected {
		return nil, nil
	}
	return []native.Device{a.device}, nil
}
func (a *fakeAudio) DriverAvailable() bool { return !a.driverMissing }
func (a *fakeAudio) DeviceRevision() uint64 {
	revision := math.Float64bits(a.device.Rate) ^ uint64(a.device.Channels)
	if a.connected {
		revision ^= 1 << 63
	}
	return revision
}
func (a *fakeAudio) SetMuted(v bool)      { a.muted = v; a.muteChanges++ }
func (a *fakeAudio) SetOverview(bool)     {}
func (a *fakeAudio) AllLevels() []float32 { return []float32{.1, .5} }
func (a *fakeAudio) Start(string, int, int, int, bool) error {
	a.starts++
	a.rates = append(a.rates, a.device.Rate)
	if a.permanent {
		return &native.AudioError{Message: "unsupported", Code: -3, Retryable: false}
	}
	if a.fail {
		return errors.New("device busy")
	}
	a.healthy = true
	a.active = "send"
	return nil
}
func (a *fakeAudio) Preview(_ string, left, right int) error {
	a.previews++
	a.left, a.right = left, right
	if a.permanent {
		return &native.AudioError{Message: "unsupported", Code: -3, Retryable: false}
	}
	if a.fail {
		return errors.New("device busy")
	}
	a.healthy = true
	a.active = "preview"
	return nil
}
func (a *fakeAudio) Stop()         { a.stops++; a.active = "" }
func (a *fakeAudio) Healthy() bool { return a.connected && a.healthy }
func (a *fakeAudio) Levels() (float32, float32, uint64, uint64) {
	a.callbacks++
	return .2, .2, a.callbacks, 0
}
func fixture(t *testing.T) (*controller, *fakeAudio, func(int)) {
	t.Helper()
	a := &fakeAudio{connected: true, healthy: true, device: native.Device{UID: "rme-stable", Name: "RME", Channels: 32, Rate: 44100}}
	now := time.Date(2026, 9, 27, 0, 0, 0, 0, time.UTC)
	config := settings.Default()
	config.DeviceUID = a.device.UID
	config.Left = 9
	c := &controller{audio: a, now: func() time.Time { return now }, retryDelay: time.Second, permissionAllowed: true, view: model{Config: config}}
	c.refresh()
	c.view.Wanted = true
	c.start()
	advance := func(ticks int) {
		for i := 0; i < ticks; i++ {
			now = now.Add(250 * time.Millisecond)
			c.tick()
		}
	}
	return c, a, advance
}
func TestRateChangesRecoverWithoutRestart(t *testing.T) {
	c, a, advance := fixture(t)
	for _, rate := range []float64{48000, 96000, 44100} {
		a.device.Rate = rate
		a.healthy = false
		advance(1)
		if c.view.Running {
			t.Fatal("must stop stale route")
		}
		advance(12)
		if !c.view.Running || a.rates[len(a.rates)-1] != rate {
			t.Fatalf("did not recover at %v: %+v", rate, c.view)
		}
	}
	if a.starts != 4 {
		t.Fatalf("unexpected restart count: %d", a.starts)
	}
	if c.view.Config.Left != 9 {
		t.Fatal("lost selected channel")
	}
}
func TestUnplugReconnectAndExplicitStop(t *testing.T) {
	c, a, advance := fixture(t)
	a.connected = false
	advance(12)
	if c.view.Running {
		t.Fatal("route still running while unplugged")
	}
	a.connected = true
	advance(12)
	if !c.view.Running {
		t.Fatal("failed to reconnect")
	}
	c.action("stop", "{}")
	starts := a.starts
	a.device.Rate = 96000
	advance(40)
	if c.view.Running || a.starts != starts {
		t.Fatal("explicit stop must cancel automatic restart")
	}
}
func TestRapidRateChangesDebounce(t *testing.T) {
	c, a, advance := fixture(t)
	a.healthy = false
	advance(1)
	for _, rate := range []float64{48000, 96000, 44100, 96000} {
		a.device.Rate = rate
		advance(2)
	}
	if a.starts != 1 {
		t.Fatal("restarted while hardware unstable")
	}
	advance(12)
	if !c.view.Running || a.starts != 2 {
		t.Fatal("did not restart exactly once after settling")
	}
}
func TestBusyHardwareRetriesWithBackoff(t *testing.T) {
	c, a, advance := fixture(t)
	a.healthy = false
	a.fail = true
	advance(120)
	if a.starts > 8 {
		t.Fatalf("retry storm: %d starts", a.starts)
	}
	a.fail = false
	advance(40)
	if !c.view.Running {
		t.Fatal("did not recover after transient errors")
	}
}
func TestRemovedChannelRequiresNewSelection(t *testing.T) {
	c, a, advance := fixture(t)
	a.healthy = false
	a.device.Channels = 8
	advance(12)
	if c.view.Running || c.view.Wanted {
		t.Fatal("must not silently route another channel")
	}
	if c.view.Config.Left != 9 {
		t.Fatal("must retain original selection")
	}
}

// Preview never needs the virtual driver, and Start/Stop switch between a
// single input-only IOProc and a send route without leaving stale meters.
func TestPreviewBeforeStartAndAfterStop(t *testing.T) {
	c, a, advance := fixture(t)
	c.action("stop", "{}")
	a.driverMissing = true
	c.refresh()
	if !c.view.Monitoring || c.view.Running || c.view.Wanted || a.active != "preview" {
		t.Fatalf("stop must leave only input metering active: %+v", c.view)
	}
	sends := a.starts
	advance(4)
	if c.view.Left >= 0 || c.view.Left <= -20 || a.starts != sends {
		t.Fatal("preview must update dBFS without sending")
	}
	a.driverMissing = false
	c.action("start", "{}")
	if !c.view.Running || c.view.Monitoring || a.active != "send" {
		t.Fatal("start must replace preview with send")
	}
	c.action("stop", "{}")
	if !c.view.Monitoring || c.view.Running || a.active != "preview" {
		t.Fatal("stopping send must resume preview")
	}
	c.action("quit", "{}")
	if c.view.Monitoring || c.view.Running || a.active != "" {
		t.Fatal("quit must release all input capture")
	}
}
func TestPreviewPermissionAndDriverIndependentStartup(t *testing.T) {
	c, a, advance := fixture(t)
	c.stop()
	c.view.Wanted = false
	c.permissionAllowed = false
	a.driverMissing = true
	previews, sends := a.previews, a.starts
	advance(12)
	if a.previews != previews {
		t.Fatal("must not capture without microphone permission")
	}
	c.action("permission_granted", "{}")
	if !c.view.Monitoring || a.starts != sends || a.active != "preview" {
		t.Fatal("permission must start preview before Start, without a driver")
	}
	c.action("permission_denied", "{}")
	advance(12)
	if c.view.Monitoring || c.view.Running || a.previews != previews+1 {
		t.Fatal("revoked permission must stop capture and retries")
	}
}
func TestPreviewSelectionAndRecovery(t *testing.T) {
	c, a, advance := fixture(t)
	c.path = t.TempDir() + "/settings.json"
	c.action("stop", "{}")
	c.action("config", `{"device_uid":"rme-stable","left":16,"right":9,"mode":"stereo","muted":true}`)
	if a.left != 15 || a.right != 8 || !c.view.Monitoring {
		t.Fatal("preview must follow both selected channels")
	}
	sends := a.starts
	a.healthy = false
	a.device.Rate = 96000
	advance(1)
	if c.view.Monitoring {
		t.Fatal("must close stale preview on rate change")
	}
	advance(12)
	if !c.view.Monitoring || a.starts != sends {
		t.Fatal("rate change must recover preview without sending")
	}
	a.connected = false
	advance(12)
	if c.view.Monitoring {
		t.Fatal("unplugged input cannot be monitored")
	}
	a.connected = true
	advance(12)
	if !c.view.Monitoring || c.view.Wanted || a.starts != sends {
		t.Fatal("reconnect must resume preview only")
	}
	a.device.Channels = 8
	a.healthy = false
	advance(12)
	if c.view.Monitoring || c.view.Config.Left != 16 {
		t.Fatal("missing channel must not silently monitor another channel")
	}
}
func TestPreviewBusyHardwareRetriesWithBackoff(t *testing.T) {
	c, a, advance := fixture(t)
	c.action("stop", "{}")
	a.healthy, a.fail = false, true
	sends := a.starts
	advance(120)
	if a.previews > 8 || a.starts != sends {
		t.Fatal("preview retry storm or unexpected send")
	}
	a.fail = false
	advance(40)
	if !c.view.Monitoring || c.view.Running {
		t.Fatal("must recover input-only metering")
	}
}

func TestFailedStartReleasesPreview(t *testing.T) {
	c, a, _ := fixture(t)
	c.action("stop", "{}")
	a.driverMissing = true
	c.action("start", "{}")
	if c.view.Monitoring || c.view.Running || a.active != "" {
		t.Fatal("failed send must not leave a stale preview blocking send recovery")
	}
}
