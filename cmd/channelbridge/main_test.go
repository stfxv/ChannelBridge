package main

import (
	"errors"
	"testing"
	"time"

	"channelbridge/internal/native"
	"channelbridge/internal/settings"
)

type fakeAudio struct {
	device                   native.Device
	connected, healthy, fail bool
	starts, stops            int
	callbacks                uint64
	rates                    []float64
}

func (a *fakeAudio) Devices() ([]native.Device, error) {
	if !a.connected {
		return nil, nil
	}
	return []native.Device{a.device}, nil
}
func (a *fakeAudio) DriverAvailable() bool { return true }
func (a *fakeAudio) Start(string, int, int, int, bool) error {
	a.starts++
	a.rates = append(a.rates, a.device.Rate)
	if a.fail {
		return errors.New("device busy")
	}
	a.healthy = true
	return nil
}
func (a *fakeAudio) Stop()         { a.stops++ }
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
	c := &controller{audio: a, now: func() time.Time { return now }, retryDelay: time.Second, view: model{Config: config}}
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
