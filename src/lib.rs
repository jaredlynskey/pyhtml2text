use html2text::config;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

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
    html: &str,
    width: usize,
    base_url: Option<String>,
    decorate: bool,
    link_footnotes: bool,
    table_borders: bool,
    wrap_links: bool,
    max_wrap_width: Option<usize>,
    raw: bool,
) -> PyResult<String> {
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
    // Rendering is pure Rust, so let other Python threads run meanwhile.
    py.detach(|| conf.string_from_read(html.as_bytes(), width))
        .map_err(|e| PyValueError::new_err(e.to_string()))
}

#[pymodule]
fn _pyhtml2text(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(render, m)?)?;
    Ok(())
}
