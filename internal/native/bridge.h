#pragma once
#include <stdint.h>

// All returned strings are heap allocated and must be freed by the caller.
char *cb_devices(void);
int cb_driver_available(void);
int cb_device_available(const char *uid);
char *cb_start(const char *uid, int left, int right, int mode, float gain, int muted);
void cb_stop(void);
void cb_levels(float *left, float *right, uint64_t *callbacks, uint64_t *faults);
int cb_healthy(void);
void cb_run(const char *model);
void cb_present(const char *model);

// Go control-plane callback. Never called from the audio real-time thread.
extern void goAction(char *action, char *payload);
