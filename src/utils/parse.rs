//! Tiny parsing helpers shared across backends.

use crate::core::error::AppError;

/// Parse a `"WxH"` string into its `(width, height)` components.
///
/// # Errors
/// [`AppError::InvalidResolutionFormat`] when the input is not `<u32>x<u32>`.
pub fn parse_wxh(value: &str) -> Result<(u32, u32), AppError> {
    let invalid = || AppError::InvalidResolutionFormat(value.to_string());
    let (w, h) = value.split_once('x').ok_or_else(invalid)?;
    let w: u32 = w.trim().parse().map_err(|_| invalid())?;
    let h: u32 = h.trim().parse().map_err(|_| invalid())?;
    Ok((w, h))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_rejects_display_sizes() {
        assert_eq!(parse_wxh("1920x1080").unwrap(), (1920, 1080));
        assert_eq!(parse_wxh(" 800 x 600 ").unwrap(), (800, 600));
        assert!(matches!(
            parse_wxh("big"),
            Err(AppError::InvalidResolutionFormat(_))
        ));
        assert!(matches!(
            parse_wxh("10x"),
            Err(AppError::InvalidResolutionFormat(_))
        ));
    }
}
