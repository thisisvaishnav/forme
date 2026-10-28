#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
mod geometry;
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
mod model;

#[cfg(target_arch = "wasm32")]
mod app;
#[cfg(target_arch = "wasm32")]
mod render;

#[cfg(target_arch = "wasm32")]
use std::cell::RefCell;
#[cfg(target_arch = "wasm32")]
use std::rc::Rc;

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::JsCast;
#[cfg(target_arch = "wasm32")]
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement};

#[cfg(target_arch = "wasm32")]
use crate::app::App;

#[cfg(target_arch = "wasm32")]
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

    let app = Rc::new(RefCell::new(App::new(window, document.clone(), canvas, ctx)));

    app.borrow_mut().attach(&app);
    App::install(&app)?;
    app.borrow_mut().load(&app);
    app.borrow_mut().draw();
    app.borrow_mut().sync_ui();
    app.borrow_mut().start_if_room();
    if let Some(root) = document.document_element() {
        let _ = root.set_attribute("data-forme-ready", "1");
    }

    Ok(())
}
