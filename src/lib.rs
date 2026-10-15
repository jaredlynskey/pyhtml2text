use std::panic::{catch_unwind, AssertUnwindSafe};

use html2text::config;
use pyo3::exceptions::{PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyString};

mod markdown;

#[cfg(not(all(
    target_env = "gnu",
    any(target_arch = "arm", target_arch = "powerpc64", target_arch = "s390x")
)))]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

/// Borrow the input without copying it; bytes are taken to be UTF-8.
fn input_bytes<'a>(html: &'a Bound<'_, PyAny>) -> PyResult<&'a [u8]> {
    if let Ok(s) = html.downcast::<PyString>() {
        Ok(s.to_str()?.as_bytes())
    } else if let Ok(b) = html.downcast::<PyBytes>() {
        Ok(b.as_bytes())
    } else {
        Err(PyTypeError::new_err(format!(
            "html must be str or bytes, not {}",
            html.get_type().name()?
        )))
    }
}

/// Run a conversion. It is pure Rust, so other Python threads run meanwhile.
/// A panic would otherwise surface as PanicException, which `except
/// Exception` doesn't catch, so it is reported as a ValueError instead.
fn convert<F>(py: Python<'_>, f: F) -> PyResult<String>
where
    F: FnOnce() -> Result<String, html2text::Error> + Send,
{
    match py.detach(|| catch_unwind(AssertUnwindSafe(|| f().map_err(|e| e.to_string())))) {
        Ok(Ok(text)) => Ok(text),
        Ok(Err(e)) => Err(PyValueError::new_err(e)),
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
    let input = input_bytes(html)?;

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

    convert(py, || conf.string_from_read(input, width))
}

/// Convert HTML to Markdown.
///
/// The output is CommonMark with GitHub's pipe tables and strikethrough:
/// ATX headings, `-` bullets, fenced code blocks and inline links and images.
/// Lines are not wrapped. Relative links and image sources are resolved
/// against `base_url` when one is given.
#[pyfunction]
#[pyo3(name = "html2markdown", signature = (html, *, base_url = None))]
fn to_markdown(
    py: Python<'_>,
    html: &Bound<'_, PyAny>,
    base_url: Option<String>,
) -> PyResult<String> {
    let input = input_bytes(html)?;
    convert(py, || markdown::from_read(input, base_url.as_deref()))
}

#[pymodule(gil_used = false)]
fn _pyhtml2text(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(render, m)?)?;
    m.add_function(wrap_pyfunction!(to_markdown, m)?)?;
    Ok(())
}
