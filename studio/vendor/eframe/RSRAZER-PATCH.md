# RsRazer's eframe 0.31.1 patch

Source: the crates.io `eframe` 0.31.1 package. The upstream MIT and Apache-2.0
licenses are included. Only `src/native/run.rs` differs from the published code.

Windows suppresses paint events for hidden windows. The original repaint
scheduler switches to `ControlFlow::Poll` before requesting a redraw and can
remain there indefinitely after Studio closes to the tray, using one CPU core.

The patch keeps the event loop in `Wait`/`WaitUntil` and services explicitly
requested hidden/minimized frames directly. Studio skips drawing while hidden,
flushes pending edits and drops its disposable graphics. Background workers do
not request repaint while hidden. Restore still wakes the native window normally.

This follows the approach in upstream's current scheduler, without an egui API
upgrade: https://github.com/emilk/egui/issues/7776 and
https://github.com/emilk/egui/issues/8326 . Remove the patch when upgrading to an
eframe version containing those fixes.

Verify a visible startup and `--background` startup, minimize/restore, close to
tray/restore (including a second invocation), and idle CPU over at least 20 s.
Both Studio and the installer use this patch; also verify installer startup.
