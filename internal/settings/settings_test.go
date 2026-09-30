package settings

import (
	"bytes"
	"encoding/json"
	"os"
	"path/filepath"
	"testing"
)

func TestValidateHardwareChannels(t *testing.T) {
	c := Default()
	c.DeviceUID = "RME"
	c.Left = 9
	if err := c.Validate(32); err != nil {
		t.Fatal(err)
	}
	if c.Validate(8) == nil {
		t.Fatal("must reject missing channels after SMUX change")
	}
	c.Left = 1
	c.Right = 99
	if err := c.Validate(1); err != nil {
		t.Fatal("mono must ignore right channel", err)
	}
	c.Mode = "stereo"
	if c.Validate(1) == nil {
		t.Fatal("stereo must validate right channel")
	}
}
func TestSaveLoad(t *testing.T) {
	path := filepath.Join(t.TempDir(), "preferences", "settings.json")
	want := Default()
	want.DeviceUID = "persistent-uid"
	want.Left = 16
	if err := Save(path, want); err != nil {
		t.Fatal(err)
	}
	got, err := Load(path)
	if err != nil || got != want {
		t.Fatalf("round trip: %+v, %v", got, err)
	}
}

// Old persisted gain settings must not survive as an invisible input adjustment.
func TestLegacyGainIsIgnored(t *testing.T) {
	path := filepath.Join(t.TempDir(), "settings.json")
	if err := os.WriteFile(path, []byte(`{"device_uid":"RME","left":5,"mode":"mono","gain_db":-42,"muted":true}`), 0600); err != nil {
		t.Fatal(err)
	}
	c, err := Load(path)
	if err != nil || c.Left != 5 || !c.Muted {
		t.Fatalf("legacy routing lost: %+v, %v", c, err)
	}
	data, err := json.Marshal(c)
	if err != nil || bytes.Contains(data, []byte("gain_db")) {
		t.Fatal("legacy gain must not be serialized")
	}
}
