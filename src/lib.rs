#![cfg(target_arch = "wasm32")]

mod app;
mod model;
mod render;

use std::cell::RefCell;
use std::rc::Rc;

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement};

use crate::app::App;

#[wasm_bindgen]
pub fn start() -> Result<(), JsValue> {
    console_error_panic_hook::set_once();

    let window = web_sys::window().ok_or_else(|| JsValue::from_str("no window"))?;
    let document = window
        .document()
        .ok_or_else(|| JsValue::from_str("no document"))?;
    let canvas = document
        .get_element_by_id("board")
        .ok_or_else(|| JsValue::from_str("missing #board"))?
        .dyn_into::<HtmlCanvasElement>()?;
    let ctx = canvas
        .get_context("2d")?
        .ok_or_else(|| JsValue::from_str("no 2d context"))?
        .dyn_into::<CanvasRenderingContext2d>()?;

    let app = Rc::new(RefCell::new(App::new(
        window,
        document,
        canvas,
        ctx,
    )));

    App::install(&app)?;
    app.borrow_mut().load(&app);
    app.borrow_mut().draw();
    app.borrow_mut().sync_ui();

    Ok(())
}
