// SPDX-License-Identifier: GPL-3.0-only
package main

import (
	"channelbridge/internal/native"
	"channelbridge/internal/settings"
	"encoding/json"
	"fmt"
	"os"
	"runtime"
	"time"
)

func main() {
	runtime.LockOSThread() // Only AppKit owns the main OS thread.
	if len(os.Args) > 1 && os.Args[1] == "--devices" {
		d, err := native.Devices()
		if err != nil {
			fmt.Fprintln(os.Stderr, err)
			os.Exit(1)
		}
		b, _ := json.MarshalIndent(d, "", "  ")
		fmt.Println(string(b))
		return
	}
	path, err := settings.Path()
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
	config, err := settings.Load(path)
	initial := model{State: stateIdle, Config: config, Left: meterFloorDB, Right: meterFloorDB, PeakLeft: meterFloorDB, PeakRight: meterFloorDB, Status: "入力を選ぶとレベルを確認できます。「開始」で送出します"}
	if err != nil {
		initial.Config = settings.Default()
		initial.Status = "保存設定を読み込めませんでした。入力を再設定してください"
	}
	c := &controller{audio: nativeAudio{}, now: time.Now, present: native.Present, path: path, retryDelay: time.Second, view: initial}
	runner := newControlRunner(c)
	native.OnAction = runner.send
	// Initial UI is a separate value; the worker exclusively owns the controller.
	b, _ := json.Marshal(initial)
	go runner.run()
	native.Run(b)
	runner.send("quit", "{}") // Also handles returning because another instance exists.
}
