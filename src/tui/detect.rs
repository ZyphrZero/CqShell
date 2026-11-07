use crate::theme::Background;
use std::{
    io::{self, IsTerminal},
    time::Duration,
};

pub fn detect(enabled: bool) -> Background {
    if !enabled {
        return Background::Disabled;
    }
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Background::Unavailable("not a terminal".into());
    }
    let mut options = terminal_colorsaurus::QueryOptions::default();
    options.timeout = Duration::from_millis(900);
    let result = terminal_colorsaurus::background_color(options);
    #[cfg(unix)]
    unsafe {
        libc::tcflush(libc::STDIN_FILENO, libc::TCIFLUSH);
    }
    match result {
        Ok(color) => Background::Detected(color.scale_to_8bit()),
        Err(error) => Background::Unavailable(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn disabled_and_nonterminal_detection_do_not_query() {
        assert_eq!(detect(false), Background::Disabled);
        if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
            assert!(
                matches!(detect(true), Background::Unavailable(reason) if reason == "not a terminal")
            );
        }
    }
}
