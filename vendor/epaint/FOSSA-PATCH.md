# epaint 0.37.0, patched for VectorCraft

The crates.io epaint 0.37.0 sources with one change in `src/text/font_face.rs`: glyphs are
rasterised on a single-threaded `vello_cpu::RenderContext` (`num_threads: 0`).

Why: epaint makes its context with `RenderContext::new`, whose thread count defaults to the cores
when any crate in the build turns on vello_cpu's `multithreading` feature (vectorcraft-render
does), and then renders without `flush()`, which the multi-threaded dispatcher asserts on
("attempted to rasterize before flushing"). Every text frame then panics.

Any program that links VectorCraft with egui 0.37 needs this patch (`[patch.crates-io] epaint`).
