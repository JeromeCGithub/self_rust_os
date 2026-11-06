# P2 execution evidence

All files in this directory come from this phase's actual host/QEMU runs.
See [architecture, reproduction and open gates](../browser-rendering-p2.md).

- Font: [initial red](font-red.log), [real bold-face red](font-bold-red.log),
  [metrics red](metrics-red.log), [green](fonts-green.log).
- Static PNG: [decoder red](image-red.log), [layout red](image-layout-red.log),
  [decoder green](images-green.log).
- CSS: [source-order/importance red](cascade-red.log).
- Native baseline: [P1 bitmap failure](font-guest-red.log),
  [P1 missing image pixels](image-guest-red.log),
  [first-load Stop failure](stop-red.log),
  [underline overwrites image pixels](decoration-red.log).
- [Adjacent black text stretches an image](image-text-red.log).
- SDK: [mask red](mask-red.log), [RGBA red](rgba-red.log),
  [25 graphics/input/protocol green tests](ui-green.log).
- [History red](history-red.log), [4 navigation/editor green tests](history-green.log).
- [Browser/cascade/image/layout/forms/prototype regressions](browser-green.log).
- [44 tooling checks](tools-green.log), [desktop](desktop-green.log),
  [native display mapping/cleanup](display-mapping-green.log).

Manual host GUI/input capture was not tested. A red/failed intermediate native
run is evidence of that failure, not a pass or a substitute for final assertions.

The [final combined native run](native-green.log) passed all enabled interactions,
including CERN over the real guest network. Screenshots captured from that tested
build: [proportional/Unicode typography](typography.png), [PNG](decoded-image.png),
[PNG alpha](alpha-image.png), [Forward destination](history-forward.png),
[real CERN page](cern-public.png). These supplement the framebuffer/input/socket
assertions. They are not modern-site or manual host GUI compatibility evidence.
