pub mod curve;

use std::time::Duration;

use smithay::reexports::rustix::time::{clock_gettime, ClockId};
use smithay::utils::{Monotonic, Time};

use self::curve::AnimationCurve;

/// A trait representing any kind of animation for the compositor needs.
#[derive(Clone, Copy, Debug)]
pub struct Animation {
    pub start: f64,
    pub end: f64,
    current_value: f64,
    curve: AnimationCurve,
    started_at: Time<Monotonic>,
    current_time: Time<Monotonic>,
    duration: Duration,
}

impl Animation {
    /// Creates a new animation with given parameters.
    ///
    /// This should be used wisely.
    pub fn new(start: f64, end: f64, curve: AnimationCurve, mut duration: Duration) -> Self {
        assert!(
            !(start == end),
            "Tried to create an animation with the same start and end!"
        );

        // This is basically the same as smithay's monotonic clock struct
        let kernel_timespec = clock_gettime(ClockId::Monotonic);
        let started_at = Duration::new(
            kernel_timespec.tv_sec as u64,
            kernel_timespec.tv_nsec as u32,
        )
        .into();

        // If we are using spring animations just ignore whatever the user puts for the duration.
        if let AnimationCurve::Spring(spring) = &curve {
            duration = spring.duration();
        }

        Self {
            start,
            end,
            current_value: start,
            curve,
            started_at,
            current_time: started_at,
            duration,
        }
    }

    /// Set the current time of the animation.
    ///
    /// This will calculate the new value at this time.
    pub fn set_current_time(&mut self, new_current_time: Time<Monotonic>) {
        self.current_time = new_current_time;
        self.current_value = match &mut self.curve {
            AnimationCurve::Simple(easing) => {
                // keyframe's easing function take an x value between [0.0, 1.0], so normalize out
                // x value to these.
                let elapsed = Time::elapsed(&self.started_at, self.current_time).as_secs_f64();
                let total = self.duration.as_secs_f64();
                let x = (elapsed / total).clamp(0., 1.);
                easing.y(x) * (self.end - self.start) + self.start
            }
            AnimationCurve::Cubic(cubic) => {
                // Cubic animations also take in X between [0.0, 1.0] and outputs a progress in
                // [0.0, 1.0]
                let elapsed = Time::elapsed(&self.started_at, self.current_time).as_secs_f64();
                let total = self.duration.as_secs_f64();
                let x = (elapsed / total).clamp(0., 1.);
                cubic.y(x) * (self.end - self.start) + self.start
            }
            AnimationCurve::Spring(spring) => {
                let elapsed = Time::elapsed(&self.started_at, self.current_time).as_secs_f64();
                spring.oscillate(elapsed) * (self.end - self.start) + self.start
            }
        };
    }

    /// Check whether the animation is finished or not.
    ///
    /// Basically checks the time.
    pub fn is_finished(&self) -> bool {
        Time::elapsed(&self.started_at, self.current_time) >= self.duration
    }

    /// Get the value at the current time
    pub fn value(&self) -> f64 {
        self.current_value
    }
}
