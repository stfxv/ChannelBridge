// SPDX-License-Identifier: GPL-3.0-only
package main

import (
	"fmt"
	"math"
	"strings"
	"time"

	"channelbridge/internal/native"
	"channelbridge/internal/settings"
)

// Finite JSON sentinel for silence. The UI renders it as −∞ dBFS.
const meterFloorDB float32 = -120

// Hold the largest displayed peak for three seconds, independently of live dBFS.
// Expiry uses a clock, not UI ticks, so delayed snapshots cannot extend a peak.
type heldPeak struct {
	db    float32
	until time.Time
}

func (p *heldPeak) update(db float32, now time.Time) float32 {
	if !now.Before(p.until) || db >= p.db {
		p.db, p.until = db, now.Add(3*time.Second)
	}
	return p.db
}

func peakDBFS(peak float32) float32 {
	if peak <= 0 || math.IsNaN(float64(peak)) || math.IsInf(float64(peak), 0) {
		return meterFloorDB
	}
	return float32(math.Max(float64(meterFloorDB), 20*math.Log10(float64(peak))))
}

func channelBits(device native.Device, channel int) string {
	for _, format := range device.Formats {
		if format.Bits > 0 && channel >= format.FirstChannel && channel < format.FirstChannel+format.Channels {
			kind := "整数"
			if format.Float {
				kind = "float"
			}
			return fmt.Sprintf("%d bit %s", format.Bits, kind)
		}
	}
	return "bit 取得不可"
}

func inputFormat(device native.Device, config settings.Config) string {
	rate := "周波数取得不可"
	if device.Rate > 0 {
		rate = fmt.Sprintf("%g kHz", device.Rate/1000)
	}
	channels := []string{fmt.Sprintf("Ch %d: %s", config.Left, channelBits(device, config.Left))}
	if config.Mode != "mono" && config.Right != config.Left {
		channels = append(channels, fmt.Sprintf("Ch %d: %s", config.Right, channelBits(device, config.Right)))
	}
	return "入力: " + rate + " / " + strings.Join(channels, " · ")
}
