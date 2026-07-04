// This file is based on code from:
//   https://github.com/rustwasm/wasm-bindgen/blob/main/examples/wasm-audio-worklet/src/dependent_module.rs
//
// The original code is licensed under either of:
//   - MIT license (https://opensource.org/licenses/MIT)
//   - Apache License, Version 2.0 (https://www.apache.org/licenses/LICENSE-2.0)
// at your option.
//
// Copyright (c) 2017-2024 The wasm-bindgen Developers
//
// This file incorporates code from the above source under the Apache License, Version 2.0 license.
// Please see the original repository for more details.
//
// See this issue for a further explanation of what this file does: https://github.com/rustwasm/wasm-bindgen/issues/3019

use js_sys::{wasm_bindgen, Array, JsString};
use wasm_bindgen::prelude::*;
use web_sys::{Blob, BlobPropertyBag, Url};

// This is a not-so-clean approach to get the current bindgen ES module URL
// in Rust. This will fail at run time on bindgen targets not using ES modules.
#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen]
    type ImportMeta;

    #[wasm_bindgen(method, getter)]
    fn url(this: &ImportMeta) -> JsString;

    #[wasm_bindgen(thread_local_v2, js_namespace = import, js_name = meta)]
    static IMPORT_META: ImportMeta;
}

pub fn on_the_fly(code: &str) -> Result<String, JsValue> {
    // TextEncoder/TextDecoder are not available in AudioWorkletGlobalScope, but
    // wasm-bindgen's generated glue constructs a TextDecoder at module-init
    // time. Load a tiny polyfill module *before* the bindgen module: ES module
    // dependencies evaluate in source order, so the globals exist by the time
    // bindgen runs.
    let polyfill_url = blob_url(&[TEXT_CODEC_POLYFILL])?;

    // Generate the import of the bindgen ES module, assuming `--target web`.
    let header = format!(
        "import '{}';\nimport init, * as bindgen from '{}';\n\n",
        polyfill_url,
        IMPORT_META.with(ImportMeta::url),
    );

    blob_url(&[header.as_str(), code])
}

// Create a `text/javascript` blob URL from the given source parts.
fn blob_url(parts: &[&str]) -> Result<String, JsValue> {
    let options = BlobPropertyBag::new();
    options.set_type("text/javascript");
    let arr = Array::new();
    for p in parts {
        arr.push(&JsValue::from(*p));
    }
    Url::create_object_url_with_blob(&Blob::new_with_str_sequence_and_options(&arr, &options)?)
}

// Minimal UTF-8 TextEncoder/TextDecoder for AudioWorkletGlobalScope. Only
// encode()/decode() are needed (wasm-bindgen supplies its own encodeInto
// fallback). Guarded so it never shadows the native implementations.
const TEXT_CODEC_POLYFILL: &str = r#"
(function () {
  const G = globalThis;
  if (typeof G.TextEncoder === 'undefined') {
    G.TextEncoder = class TextEncoder {
      get encoding() { return 'utf-8'; }
      encode(str) {
        str = String(str);
        const out = [];
        for (let i = 0; i < str.length; i++) {
          let c = str.charCodeAt(i);
          if (c >= 0xd800 && c <= 0xdbff && i + 1 < str.length) {
            const c2 = str.charCodeAt(i + 1);
            if (c2 >= 0xdc00 && c2 <= 0xdfff) { c = 0x10000 + ((c - 0xd800) << 10) + (c2 - 0xdc00); i++; }
          }
          if (c < 0x80) out.push(c);
          else if (c < 0x800) out.push(0xc0 | (c >> 6), 0x80 | (c & 0x3f));
          else if (c < 0x10000) out.push(0xe0 | (c >> 12), 0x80 | ((c >> 6) & 0x3f), 0x80 | (c & 0x3f));
          else out.push(0xf0 | (c >> 18), 0x80 | ((c >> 12) & 0x3f), 0x80 | ((c >> 6) & 0x3f), 0x80 | (c & 0x3f));
        }
        return new Uint8Array(out);
      }
    };
  }
  if (typeof G.TextDecoder === 'undefined') {
    G.TextDecoder = class TextDecoder {
      constructor(label, options) { this.encoding = 'utf-8'; this.fatal = !!(options && options.fatal); this.ignoreBOM = !!(options && options.ignoreBOM); }
      decode(input) {
        if (input === undefined) return '';
        const bytes = input instanceof Uint8Array ? input : new Uint8Array(input.buffer || input);
        let out = '';
        for (let i = 0; i < bytes.length;) {
          let b = bytes[i++], cp;
          if (b < 0x80) cp = b;
          else if (b < 0xe0) { cp = ((b & 0x1f) << 6) | (bytes[i++] & 0x3f); }
          else if (b < 0xf0) { cp = ((b & 0x0f) << 12) | ((bytes[i++] & 0x3f) << 6) | (bytes[i++] & 0x3f); }
          else { cp = ((b & 0x07) << 18) | ((bytes[i++] & 0x3f) << 12) | ((bytes[i++] & 0x3f) << 6) | (bytes[i++] & 0x3f); }
          if (cp > 0xffff) { cp -= 0x10000; out += String.fromCharCode(0xd800 + (cp >> 10), 0xdc00 + (cp & 0x3ff)); }
          else out += String.fromCharCode(cp);
        }
        return out;
      }
    };
  }
})();
"#;

// dependent_module! takes a local file name to a JS module as input and
// returns a URL to a slightly modified module in run time. This modified module
// has an additional import statement in the header that imports the current
// bindgen JS module under the `bindgen` alias, and the separate init function.
// How this URL is produced does not matter for the macro user. on_the_fly
// creates a blob URL in run time. A better, more sophisticated solution
// would add wasm_bindgen support to put such a module in pkg/ during build time
// and return a URL to this file instead (described in #3019).
#[macro_export]
macro_rules! dependent_module {
    ($file_name:expr) => {
        $crate::host::audioworklet::dependent_module::on_the_fly(include_str!($file_name))
    };
}
