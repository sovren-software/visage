---
name: Reliability Report
about: Face unlock works but fails too often — share your success rate from `visage stats`
title: "[Reliability] "
labels: hardware
assignees: ''
---

## Numbers

Run this after at least a week of normal use, and paste the output:

```
sudo visage stats --json
```

<!-- It contains counts, failure causes and times of day only: no names, no images,
     no face data. Check it before you post if you want to be sure. -->

## Device info

- **Laptop:** <!-- brand and model -->
- **VID:PID:** <!-- from `visage discover` -->
- **IR emitter quirk:** <!-- yes / no quirk (from `visage discover`) -->
- **Visage version:** <!-- `visage --version` -->
- **OS and desktop:** <!-- e.g. Fedora 44, GNOME on Wayland -->

## Where and when it fails

<!-- Anything you noticed: mornings, a window behind you, after resume, with glasses,
     at the lock screen only... -->

## Camera check (optional)

<!-- Output of `sudo visage test -n 20` in the place where it fails most. -->
