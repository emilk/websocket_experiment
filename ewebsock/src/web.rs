use crate::{EventHandler, Result, WsEvent, WsMessage};

macro_rules! console_log {
    ($($t:tt)*) => (web_sys::console::log_1(&format!($($t)*).into()))
}

fn string_from_js_value(s: wasm_bindgen::JsValue) -> String {
    s.as_string().unwrap_or(format!("{:#?}", s))
}

fn string_from_js_string(s: js_sys::JsString) -> String {
    s.as_string().unwrap_or(format!("{:#?}", s))
}

#[derive(Clone)]
pub struct WsSender {
    ws: web_sys::WebSocket,
}

impl WsSender {
    pub fn send(&mut self, msg: WsMessage) {
        let result = match msg {
            WsMessage::Binary(data) => {
                self.ws.set_binary_type(web_sys::BinaryType::Blob);
                self.ws.send_with_u8_array(&data)
            }
            WsMessage::Text(text) => self.ws.send_with_str(&text),
            unknown => {
                panic!("Don't know how to send message: {:?}", unknown);
            }
        };
        if let Err(err) = result.map_err(string_from_js_value) {
            tracing::error!("Failed to send: {:?}", err);
        }
    }
}

pub fn ws_connect(url: String, on_event: EventHandler) -> Result<WsSender> {
    // Based on https://rustwasm.github.io/wasm-bindgen/examples/websockets.html

    console_log!("spawn_ws_client");
    use wasm_bindgen::closure::Closure;
    use wasm_bindgen::JsCast as _;

    // Connect to an server
    let ws = web_sys::WebSocket::new(&url).map_err(string_from_js_value)?;

    // For small binary messages, like CBOR, Arraybuffer is more efficient than Blob handling
    ws.set_binary_type(web_sys::BinaryType::Arraybuffer);

    // onmessage callback
    {
        let on_event = on_event.clone();
        let onmessage_callback = Closure::wrap(Box::new(move |e: web_sys::MessageEvent| {
            // Handle difference Text/Binary,...
            if let Ok(abuf) = e.data().dyn_into::<js_sys::ArrayBuffer>() {
                console_log!("message event, received arraybuffer: {:?}", abuf);
                let array = js_sys::Uint8Array::new(&abuf);
                let len = array.byte_length() as usize;
                console_log!("Arraybuffer received {} bytes: {:?}", len, array.to_vec());
                on_event(WsEvent::Message(WsMessage::Binary(array.to_vec())));
            } else if let Ok(blob) = e.data().dyn_into::<web_sys::Blob>() {
                console_log!("message event, received blob: {:?}", blob);
                // better alternative to juggling with FileReader is to use https://crates.io/crates/gloo-file
                let fr = web_sys::FileReader::new().unwrap();
                let fr_c = fr.clone();
                // create onLoadEnd callback
                let on_event = on_event.clone();
                let onloadend_cb = Closure::wrap(Box::new(move |_e: web_sys::ProgressEvent| {
                    let array = js_sys::Uint8Array::new(&fr_c.result().unwrap());
                    let len = array.byte_length() as usize;
                    console_log!("Blob received {} bytes: {:?}", len, array.to_vec());
                    on_event(WsEvent::Message(WsMessage::Binary(array.to_vec())));
                })
                    as Box<dyn FnMut(web_sys::ProgressEvent)>);
                fr.set_onloadend(Some(onloadend_cb.as_ref().unchecked_ref()));
                fr.read_as_array_buffer(&blob).expect("blob not readable");
                onloadend_cb.forget();
            } else if let Ok(txt) = e.data().dyn_into::<js_sys::JsString>() {
                console_log!("message event, received Text: {:?}", txt);
                on_event(WsEvent::Message(WsMessage::Text(string_from_js_string(
                    txt,
                ))));
            } else {
                console_log!("message event, received Unknown: {:?}", e.data());
                on_event(WsEvent::Message(WsMessage::Unknown(string_from_js_value(
                    e.data(),
                ))));
            }
        }) as Box<dyn FnMut(web_sys::MessageEvent)>);

        // set message event handler on WebSocket
        ws.set_onmessage(Some(onmessage_callback.as_ref().unchecked_ref()));

        // forget the callback to keep it alive
        onmessage_callback.forget();
    }

    {
        let on_event = on_event.clone();
        let onerror_callback = Closure::wrap(Box::new(move |error_event: web_sys::ErrorEvent| {
            console_log!("error event: {:?}", error_event);
            on_event(WsEvent::Error(error_event.message()));
        }) as Box<dyn FnMut(web_sys::ErrorEvent)>);
        ws.set_onerror(Some(onerror_callback.as_ref().unchecked_ref()));
        onerror_callback.forget();
    }

    {
        let onopen_callback = Closure::wrap(Box::new(move |_| {
            console_log!("socket opened");
            on_event(WsEvent::Opened);
        }) as Box<dyn FnMut(wasm_bindgen::JsValue)>);
        ws.set_onopen(Some(onopen_callback.as_ref().unchecked_ref()));
        onopen_callback.forget();
    }

    Ok(WsSender { ws })
}
