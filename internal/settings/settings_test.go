package settings

import (
	"math"
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
	c.Right = 1
	c.GainDB = math.NaN()
	if c.Validate(1) == nil {
		t.Fatal("must reject non-finite gain")
	}
}
func TestSaveLoad(t *testing.T) {
	path := filepath.Join(t.TempDir(), "preferences", "settings.json")
	want := Default()
	want.DeviceUID = "persistent-uid"
	want.Left = 16
	want.GainDB = -7
	if err := Save(path, want); err != nil {
		t.Fatal(err)
	}
	got, err := Load(path)
	if err != nil || got != want {
		t.Fatalf("round trip: %+v, %v", got, err)
	}
}
