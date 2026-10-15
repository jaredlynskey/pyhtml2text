use std::panic::{catch_unwind, AssertUnwindSafe};

use html2text::config;
use pyo3::exceptions::{PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyString};

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

/// Render HTML as plain text.
#[pyfunction]
#[pyo3(name = "html2text", signature = (
    html,
    width = 80,
    *,
    base_url = None,
    decorate = true,
    link_footnotes = true,
    table_borders = true,
    wrap_links = true,
    max_wrap_width = None,
    raw = false,
))]
#[allow(clippy::too_many_arguments)]
fn render(
    py: Python<'_>,
    html: &Bound<'_, PyAny>,
    width: usize,
    base_url: Option<String>,
    decorate: bool,
    link_footnotes: bool,
    table_borders: bool,
    wrap_links: bool,
    max_wrap_width: Option<usize>,
    raw: bool,
) -> PyResult<String> {
    // Borrow the input without copying it; bytes are taken to be UTF-8.
    let input: &[u8] = if let Ok(s) = html.downcast::<PyString>() {
        s.to_str()?.as_bytes()
    } else if let Ok(b) = html.downcast::<PyBytes>() {
        b.as_bytes()
    } else {
        return Err(PyTypeError::new_err(format!(
            "html must be str or bytes, not {}",
            html.get_type().name()?
        )));
    };

    let mut conf = if decorate {
        config::plain()
    } else {
        config::plain_no_decorate()
    };
    conf = conf.link_footnotes(link_footnotes).allow_width_overflow();
    if let Some(base) = base_url {
        conf = conf.base_url(base);
    }
    if !table_borders {
        conf = conf.no_table_borders();
    }
    if !wrap_links {
        conf = conf.no_link_wrapping();
    }
    if let Some(w) = max_wrap_width {
        conf = conf.max_wrap_width(w);
    }
    if raw {
        conf = conf.raw_mode(true);
    }

    // Rendering is pure Rust, so let other Python threads run meanwhile. A
    // panic would otherwise surface as PanicException, which `except
    // Exception` doesn't catch, so report it as a ValueError instead.
    match py.detach(|| catch_unwind(AssertUnwindSafe(|| conf.string_from_read(input, width)))) {
        Ok(Ok(text)) => Ok(text),
        Ok(Err(e)) => Err(PyValueError::new_err(e.to_string())),
        Err(panic) => {
            let msg = panic
                .downcast_ref::<&str>()
                .copied()
                .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
                .unwrap_or("unknown error");
            Err(PyValueError::new_err(format!(
                "html2text failed on this document: {msg}"
            )))
        }
    }
}

#[pymodule(gil_used = false)]
fn _pyhtml2text(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(render, m)?)?;
    Ok(())
}
