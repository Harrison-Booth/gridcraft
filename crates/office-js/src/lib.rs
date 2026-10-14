//! GridCraft's implementation of the Office JavaScript API for Office add-ins' task panes.
//!
//! An Office add-in's task pane is a web page that calls `Office.onReady`, `Excel.run` and the
//! proxy objects (`context.workbook.worksheets.getItem("Sheet1").getRange("A1").values = …`,
//! `range.load("values")`, `context.sync()`). GridCraft never loads Microsoft's `office.js`:
//! this crate, compiled to wasm, defines those globals itself, written from the public API
//! reference on learn.microsoft.com (clean room). Each `context.sync()` sends the queued
//! actions as one JSON batch to the host, which runs it as `office.sync` (one undo step) and
//! answers with the loaded values or an error:
//!
//! - request `{"id": n, "actions": [{"op": "load"|"set"|"call", "path": [...], …}]}`, posted
//!   with `globalThis.__craftOffice.post(json)`;
//! - answer `{"id": n, "results": [...]}` or `{"id": n, "error": {"code", "message", "action"}}`,
//!   delivered with `globalThis.__craftOfficeReply(json)`.
//!
//! [`core`] is the batching and the cache of loaded properties (plain Rust, tested natively);
//! the JavaScript side (`js`, wasm only) is the globals and proxy classes. What isn't covered
//! throws an error with code `NotImplemented`.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

pub mod core;
#[cfg(target_arch = "wasm32")]
mod js;
#[cfg(target_arch = "wasm32")]
pub use js::*;
