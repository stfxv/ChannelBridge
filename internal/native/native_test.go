//go:build darwin

package native

import "testing"

// Read-only integration test: exercise Apple's real UID translation API, because
// a fake driver host cannot catch incorrect client-side qualifier arguments.
func TestEnumeratedDeviceUIDsResolve(t *testing.T) {
	devices, err := Devices()
	if err != nil {
		t.Fatal(err)
	}
	if len(devices) == 0 {
		t.Skip("no input devices on this machine")
	}
	for _, d := range devices {
		if !DeviceAvailable(d.UID) {
			t.Errorf("enumerated UID does not resolve: %s", d.Name)
		}
	}
	if DeviceAvailable("local.channelbridge.nonexistent-test-device") {
		t.Fatal("unknown UID unexpectedly resolved")
	}
}
