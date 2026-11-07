//! Time-based motion primitives. Every function takes explicit instants so drawing stays
//! deterministic in tests; the event loop supplies the clock once per frame.
use std::time::{Duration, Instant};

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const SPINNER_STEP: Duration = Duration::from_millis(80);
/// Gauge bars glide to a new sample instead of jumping.
pub(super) const GLIDE: Duration = Duration::from_millis(450);
/// Transient feedback stays readable for this long, then fades into the border color.
pub(super) const TOAST_LIFETIME: Duration = Duration::from_millis(2800);
const TOAST_FADE: Duration = Duration::from_millis(500);
/// Frame budgets: smooth while something moves, relaxed while only a spinner turns.
pub(super) const ANIMATION_FRAME: Duration = Duration::from_millis(33);
pub(super) const SPINNER_FRAME: Duration = SPINNER_STEP;
pub(super) const IDLE_FRAME: Duration = Duration::from_millis(250);
/// Background reads shorter than this open their form without showing a loading dialog.
pub(super) const LOADING_DELAY: Duration = Duration::from_millis(150);
pub(super) const DIALOG_FADE: Duration = Duration::from_millis(120);

pub(super) fn dialog_opacity(age: Duration) -> f64 {
    (age.as_secs_f64() / DIALOG_FADE.as_secs_f64()).clamp(0.0, 1.0)
}

pub(super) fn spinner(elapsed: Duration) -> &'static str {
    let step = (elapsed.as_millis() / SPINNER_STEP.as_millis()) as usize;
    SPINNER[step % SPINNER.len()]
}

fn ease_out_cubic(t: f64) -> f64 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Tween {
    from: f64,
    to: f64,
    started: Instant,
}
impl Tween {
    /// A new gauge grows from empty, which reads as the panel coming alive.
    pub fn new(to: f64, now: Instant) -> Self {
        Self {
            from: 0.0,
            to,
            started: now,
        }
    }
    pub fn value(&self, now: Instant) -> f64 {
        let t = now.saturating_duration_since(self.started).as_secs_f64() / GLIDE.as_secs_f64();
        self.from + (self.to - self.from) * ease_out_cubic(t)
    }
    pub fn moving(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.started) < GLIDE
    }
    /// Restart from the currently displayed value; sub-cell noise does not restart the glide.
    pub fn retarget(&mut self, to: f64, now: Instant) {
        if (to - self.to).abs() < 0.05 {
            return;
        }
        self.from = self.value(now);
        self.to = to;
        self.started = now;
    }
}

/// Full intensity until the final fade window, then linear fade to zero at expiry.
pub(super) fn toast_opacity(age: Duration) -> f64 {
    if age >= TOAST_LIFETIME {
        return 0.0;
    }
    let remaining = TOAST_LIFETIME - age;
    if remaining < TOAST_FADE {
        remaining.as_secs_f64() / TOAST_FADE.as_secs_f64()
    } else {
        1.0
    }
}
pub(super) fn toast_fading(age: Duration) -> bool {
    age < TOAST_LIFETIME && TOAST_LIFETIME - age < TOAST_FADE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spinner_advances_with_time_and_wraps() {
        assert_eq!(spinner(Duration::ZERO), SPINNER[0]);
        assert_eq!(spinner(SPINNER_STEP), SPINNER[1]);
        assert_eq!(spinner(SPINNER_STEP * SPINNER.len() as u32), SPINNER[0]);
    }

    #[test]
    fn tween_eases_toward_target_and_retargets_from_the_displayed_value() {
        let start = Instant::now();
        let mut tween = Tween::new(80.0, start);
        assert_eq!(tween.value(start), 0.0);
        let midway = tween.value(start + GLIDE / 2);
        assert!(
            midway > 40.0 && midway < 80.0,
            "ease-out front-loads motion"
        );
        assert!((tween.value(start + GLIDE) - 80.0).abs() < 1e-9);
        assert!(!tween.moving(start + GLIDE));

        let later = start + GLIDE * 2;
        tween.retarget(20.0, later);
        assert!((tween.value(later) - 80.0).abs() < 1e-9);
        assert!(tween.moving(later));
        assert!((tween.value(later + GLIDE) - 20.0).abs() < 1e-9);

        let settled = later + GLIDE * 2;
        tween.retarget(20.01, settled);
        assert!(
            !tween.moving(settled),
            "noise below the threshold keeps the bar still"
        );
    }

    #[test]
    fn toast_holds_then_fades_out_before_expiry() {
        assert_eq!(toast_opacity(Duration::ZERO), 1.0);
        assert!(!toast_fading(Duration::ZERO));
        let fading = TOAST_LIFETIME - TOAST_FADE / 2;
        assert!((toast_opacity(fading) - 0.5).abs() < 1e-9);
        assert!(toast_fading(fading));
        assert_eq!(toast_opacity(TOAST_LIFETIME), 0.0);
        assert!(!toast_fading(TOAST_LIFETIME));
    }
}
