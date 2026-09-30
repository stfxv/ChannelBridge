package main

import (
	"math"
	"strings"
	"testing"

	"channelbridge/internal/native"
	"channelbridge/internal/settings"
)

func TestPeakDBFS(t *testing.T) {
	for _, test := range []struct{ peak, want float32 }{
		{1, 0}, {.5, -6.0206}, {.1, -20}, {0.00001, -100}, {2, 6.0206}, {0, meterFloorDB},
		{float32(math.NaN()), meterFloorDB}, {float32(math.Inf(1)), meterFloorDB},
	} {
		got := peakDBFS(test.peak)
		if math.Abs(float64(got-test.want)) > .001 {
			t.Errorf("peak %v: got %v, want %v", test.peak, got, test.want)
		}
	}
}
func TestInputFormatUsesSelectedPhysicalStream(t *testing.T) {
	d := native.Device{Rate: 44100, Formats: []native.StreamFormat{
		{FirstChannel: 1, Channels: 8, Bits: 24},
		{FirstChannel: 9, Channels: 8, Bits: 32, Float: true},
	}}
	c := settings.Config{Left: 9, Right: 2, Mode: "stereo"}
	got := inputFormat(d, c)
	for _, want := range []string{"44.1 kHz", "Ch 9: 32 bit float", "Ch 2: 24 bit 整数"} {
		if !strings.Contains(got, want) {
			t.Errorf("missing %q in %q", want, got)
		}
	}
	c.Mode = "mono"
	if strings.Contains(inputFormat(d, c), "Ch 2") {
		t.Fatal("mono must only show its selected input")
	}
	if channelBits(d, 17) != "bit 取得不可" {
		t.Fatal("unknown stream must not show guessed bit depth")
	}
	d.Rate = 96000
	if !strings.Contains(inputFormat(d, c), "96 kHz") {
		t.Fatal("format display must follow rate change")
	}
}
func TestControllerReportsDBFSAndClearsOnStop(t *testing.T) {
	c, _, advance := fixture(t)
	advance(1)
	if math.Abs(float64(c.view.Left)-(-13.9794)) > .001 {
		t.Fatalf("level must be dBFS: %v", c.view.Left)
	}
	c.stop()
	if c.view.Left != meterFloorDB || c.view.Right != meterFloorDB {
		t.Fatal("stopped meters must clear")
	}
}
