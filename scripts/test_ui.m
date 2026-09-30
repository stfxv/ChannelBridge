// Read-only AppKit regression: no microphone permission, capture, or driver I/O.
#include <assert.h>
#include <stdio.h>
#import "../internal/native/ui_darwin.m"

static NSDictionary *lastConfig;
@interface CBTestApp : CBApp @end
@implementation CBTestApp
- (void)checkPermission {} // UI selection tests must never request microphone access.
@end
void goAction(char *name, char *payload) {
    if (!strcmp(name, "config"))
        lastConfig = [NSJSONSerialization JSONObjectWithData:[[NSString stringWithUTF8String:payload] dataUsingEncoding:NSUTF8StringEncoding] options:0 error:nil];
}

int main(void) {
    @autoreleasepool {
        [NSApplication sharedApplication];
        CBApp *app = [CBTestApp new];
        app.model = @{@"config": @{@"device_uid": @"test-device", @"left": @9, @"right": @16, @"mode": @"stereo", @"muted": @NO}};
        app.devices = popup(NSZeroRect, app);
        [app.devices addItemWithTitle:@"Test device"];
        app.devices.lastItem.representedObject = @"test-device";
        app.left = popup(NSZeroRect, app); app.right = popup(NSZeroRect, app);
        app.mode = popup(NSZeroRect, app);
        [app.mode addItemsWithTitles:@[@"mono", @"stereo", @"mix"]];
        [app.mode selectItemAtIndex:1];
        app.mute = [NSButton checkboxWithTitle:@"mute" target:nil action:nil];
        // 32ch -> 8ch while saved Ch9/Ch16 disappear. Both remain selected.
        updateChannels(app.left, 32, 9); updateChannels(app.right, 32, 16);
        updateChannels(app.left, 8, 9); updateChannels(app.right, 8, 16);
        assert(app.left.selectedItem.tag == 9 && app.right.selectedItem.tag == 16);
        assert(!app.left.selectedItem.enabled && !app.right.selectedItem.enabled);
        app.mute.state = NSControlStateValueOn;
        [app changed:app.mute];
        assert([lastConfig[@"left"] intValue] == 9 && [lastConfig[@"right"] intValue] == 16);
        assert([lastConfig[@"muted"] boolValue]);
        // Only an explicit channel selection may replace a missing selection.
        [app.left selectItemWithTag:2];
        [app changed:app.left];
        assert([lastConfig[@"left"] intValue] == 2 && [lastConfig[@"right"] intValue] == 16);
        updateChannels(app.left, 32, 9);
        assert(app.left.selectedItem.tag == 9 && app.left.selectedItem.enabled);
        updateChannels(app.left, 0, 9);
        assert(app.left.selectedItem.tag == 9);
        puts("UI: missing channels survive menu rebuild, mute, explicit selection and reconnect");
    }
}
