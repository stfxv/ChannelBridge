//go:build darwin

// Package native is the small boundary to AppKit and Core Audio.
// Audio callbacks stay entirely in C; Go owns validation and preferences.
package native

/*
#cgo CFLAGS: -fobjc-arc -mmacosx-version-min=13.0
#cgo LDFLAGS: -framework Cocoa -framework CoreAudio -framework AVFoundation -framework Foundation
#include <stdlib.h>
#include "bridge.h"
*/
import "C"

import (
	"encoding/json"
	"fmt"
	"unsafe"
)

type Device struct {
	UID      string         `json:"uid"`
	Name     string         `json:"name"`
	Channels int            `json:"channels"`
	Rate     float64        `json:"rate"`
	Formats  []StreamFormat `json:"formats"`
}

// StreamFormat is the input format reported by the HAL driver, not an ADC specification.
type StreamFormat struct {
	FirstChannel int  `json:"first_channel"`
	Channels     int  `json:"channels"`
	Bits         int  `json:"bits"`
	Float        bool `json:"float"`
}

func Devices() ([]Device, error) {
	p := C.cb_devices()
	defer C.free(unsafe.Pointer(p))
	var d []Device
	err := json.Unmarshal([]byte(C.GoString(p)), &d)
	return d, err
}
func DriverAvailable() bool { return C.cb_driver_available() != 0 }
func Start(uid string, left, right, mode int, muted bool) error {
	p := C.CString(uid)
	defer C.free(unsafe.Pointer(p))
	m := 0
	if muted {
		m = 1
	}
	e := C.cb_start(p, C.int(left), C.int(right), C.int(mode), C.int(m))
	if e == nil {
		return nil
	}
	defer C.free(unsafe.Pointer(e))
	return fmt.Errorf("%s", C.GoString(e))
}
func Stop()         { C.cb_stop() }
func Healthy() bool { return C.cb_healthy() != 0 }
func Levels() (float32, float32, uint64, uint64) {
	var l, r C.float
	var n, f C.uint64_t
	C.cb_levels(&l, &r, &n, &f)
	return float32(l), float32(r), uint64(n), uint64(f)
}

var OnAction func(string, string)

//export goAction
func goAction(action, payload *C.char) {
	if OnAction != nil {
		OnAction(C.GoString(action), C.GoString(payload))
	}
}
func Run(model []byte) { p := C.CString(string(model)); defer C.free(unsafe.Pointer(p)); C.cb_run(p) }
func Present(model []byte) {
	p := C.CString(string(model))
	defer C.free(unsafe.Pointer(p))
	C.cb_present(p)
}

// DeviceAvailable resolves the persisted UID through HAL (no recording).
func DeviceAvailable(uid string) bool {
	p := C.CString(uid)
	defer C.free(unsafe.Pointer(p))
	return C.cb_device_available(p) != 0
}
