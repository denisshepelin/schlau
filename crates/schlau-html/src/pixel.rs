use crate::css::declarations;

/// The attributes of an `<img>` that decide whether it is a tracking pixel by shape.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct ImageShape<'a> {
    pub width: Option<&'a str>,
    pub height: Option<&'a str>,
    pub style: Option<&'a str>,
    pub hidden: bool,
}

fn pixels(value: &str) -> Option<f32> {
    let value = value.trim();
    let number = value.strip_suffix("px").unwrap_or(value).trim();
    (!number.ends_with('%'))
        .then(|| number.parse::<f32>().ok())
        .flatten()
}

#[derive(Debug, Default)]
struct StyleFacts {
    width: Option<f32>,
    height: Option<f32>,
    hidden: bool,
}

fn style_facts(style: &str) -> StyleFacts {
    declarations(style).fold(StyleFacts::default(), |facts, (name, value)| {
        match name.as_str() {
            "display" if value == "none" => StyleFacts {
                hidden: true,
                ..facts
            },
            "visibility" if value == "hidden" || value == "collapse" => StyleFacts {
                hidden: true,
                ..facts
            },
            "opacity" => {
                let opacity = value.strip_suffix('%').map_or_else(
                    || value.parse::<f32>().ok(),
                    |p| p.parse::<f32>().ok().map(|p| p / 100.0),
                );
                StyleFacts {
                    hidden: facts.hidden || opacity.is_some_and(|o| o <= 0.01),
                    ..facts
                }
            }
            "width" | "max-width" => StyleFacts {
                width: pixels(&value)
                    .map(|w| facts.width.map_or(w, |old| old.min(w)))
                    .or(facts.width),
                ..facts
            },
            "height" | "max-height" => StyleFacts {
                height: pixels(&value)
                    .map(|h| facts.height.map_or(h, |old| old.min(h)))
                    .or(facts.height),
                ..facts
            },
            _ => facts,
        }
    })
}

fn is_tiny(width: Option<f32>, height: Option<f32>) -> bool {
    let any_hairline = [width, height].iter().flatten().any(|d| *d <= 1.0);
    let both_small = matches!((width, height), (Some(w), Some(h)) if w <= 3.0 && h <= 3.0);
    any_hairline || both_small
}

/// True for images that a reader would never see: hidden or at most a few pixels.
pub(crate) fn is_invisible(shape: ImageShape<'_>) -> bool {
    let facts = shape.style.map(style_facts).unwrap_or_default();
    let width = facts.width.or_else(|| shape.width.and_then(pixels));
    let height = facts.height.or_else(|| shape.height.and_then(pixels));
    shape.hidden || facts.hidden || is_tiny(width, height)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape<'a>(
        width: Option<&'a str>,
        height: Option<&'a str>,
        style: Option<&'a str>,
    ) -> ImageShape<'a> {
        ImageShape {
            width,
            height,
            style,
            hidden: false,
        }
    }

    #[test]
    fn detects_pixels() {
        assert!(is_invisible(shape(Some("1"), Some("1"), None)));
        assert!(is_invisible(shape(Some("0"), None, None)));
        assert!(is_invisible(shape(
            None,
            None,
            Some("width:1px;height:1px")
        )));
        assert!(is_invisible(shape(
            Some("600"),
            Some("300"),
            Some("display: none !important")
        )));
        assert!(is_invisible(shape(None, None, Some("opacity:0"))));
        assert!(is_invisible(shape(Some("2"), Some("3"), None)));
    }

    #[test]
    fn keeps_real_images() {
        assert!(!is_invisible(shape(Some("600"), Some("300"), None)));
        assert!(!is_invisible(shape(
            Some("100%"),
            None,
            Some("max-width:600px")
        )));
        assert!(!is_invisible(shape(None, None, None)));
        assert!(!is_invisible(shape(
            Some("24"),
            Some("24"),
            Some("opacity:0.8")
        )));
    }
}
