// SPDX-License-Identifier: GPL-3.0-only
package main

import (
	"runtime"
	"sync"
	"time"
)

type controlAction struct{ name, payload string }

// AppKit only enqueues actions. One worker owns the controller and every HAL
// operation, so a slow route rebuild cannot block the UI or race a second rebuild.
type controlRunner struct {
	controller *controller
	mu         sync.Mutex
	queue      []controlAction
	closed     bool
	wake       chan struct{}
	done       chan struct{}
}

func newControlRunner(c *controller) *controlRunner {
	return &controlRunner{controller: c, wake: make(chan struct{}, 1), done: make(chan struct{})}
}

func (r *controlRunner) send(name, payload string) {
	r.mu.Lock()
	if !r.closed {
		r.queue = append(r.queue, controlAction{name, payload})
	}
	r.mu.Unlock()
	select {
	case r.wake <- struct{}{}:
	default:
	}
	if name == "quit" {
		<-r.done // Release input I/O before AppKit terminates the process.
	}
}

func (r *controlRunner) drain() bool {
	for {
		r.mu.Lock()
		if len(r.queue) == 0 {
			r.mu.Unlock()
			return false
		}
		a := r.queue[0]
		r.queue[0] = controlAction{}
		r.queue = r.queue[1:]
		r.mu.Unlock()
		r.controller.action(a.name, a.payload)
		if a.name == "quit" {
			return true
		}
	}
}

func (r *controlRunner) run() {
	runtime.LockOSThread()
	defer runtime.UnlockOSThread()
	defer func() {
		r.mu.Lock()
		r.closed = true
		r.queue = nil
		r.mu.Unlock()
		close(r.done)
	}()
	r.controller.refresh()
	r.controller.publish()
	ticker := time.NewTicker(250 * time.Millisecond)
	defer ticker.Stop()
	for {
		select {
		case <-r.wake:
			if r.drain() {
				return
			}
		case <-ticker.C:
			if r.drain() {
				return
			} // User Stop/selection takes precedence over recovery.
			r.controller.tick()
		}
	}
}
