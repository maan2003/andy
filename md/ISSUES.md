# Issues

## use @e1 reference instead of clicking by text

## accessiblity tree doesn't update in webview

## auto wait is not perfect
now it is improved, doesn't throw error
but it sleeps too much, like:
`tap .. && a11y && screenshot`   
  - first tap is free and then a11y sleeps for 1100ms but then screenshot sleeps for another 750ms incorrectly

maybe doing sleep after tap is just the easy solution?
we can make it better by waiting in background thread and only blocking on next call to (tap, screenshot) and avoids blocking the thread as well.

## maybe auto screenshot/a11y after some commands? would save some tokens. or just combine a11y with screenshot (just call it view)

## add multi screen example

## Warn on screen reset- the previous one was reaped due to heartbeat timeout.

## Remove `stop` command?

## Automatic complains from claude

  1. WebView a11y tree goes stale. This was the biggest problem. After navigating within the community tool WebView
  (step 1 -> step 2), andy a11y kept returning the step 1 DOM even though screenshots clearly showed step 2. This
  forced me to rely entirely on coordinate taps and screenshots for WebView content, which is slow and fragile.

  2. andy tap "text" fails on WebView content. Even when the a11y tree did show WebView elements (like the Button
  "Confirm" in the publish dialog), andy tap "Publish" failed with node not found. It seems like text matching only
  works against native views, not WebView a11y nodes. The dialog's Confirm button showed as Button "Confirm" in a11y
   but I had to compute center coordinates manually from the bounds.

  3. No way to auto-set up adb reverse. This isn't really andy's job, but every time the emulator restarts or
  reconnects, all adb reverse mappings are lost. Having an andy reverse shortcut or auto-forwarding common dev ports
   (8081, 3022, 26722) would save a lot of pain for dev workflows.

  4. Minor: andy launch after a crash sometimes needs a retry. I hit Error: adb shell getprop failed (is a device
  connected?) once even though adb devices showed the device. A second andy launch worked fine. Might just need a
  retry internally.

