//! Why a detector refused to split an input after matching its format.
//!
//! Detectors return `None` both for an input that is not their format and for
//! one whose signature they matched but could not split safely (code around
//! the bundle a factory depends on, factories that fail normalization). The
//! caller needs the difference: a refused bundle is still a bundle, so a
//! directory walk keeps it as one file instead of skipping it as plain.
//!
//! A detector marks the point where its signature matched with [`commit`].
//! The returned [`Commitment`] records a refusal when it is dropped without
//! [`Commitment::accept`]ing a result, so every early return after that point
//! counts, including `?`. A failing check can name its reason first with
//! [`because`]; without one, the refusal reason is
//! [`SplitRefusalReason::Other`].
//!
//! Records go to the innermost active [`capture`] on the current thread and
//! are dropped when none is active. Detection of one input runs on one
//! thread. The first refusal wins: detectors run in priority order.

use std::cell::RefCell;

use super::BundleFormat;

/// What made a detector refuse an input it had matched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitRefusalReason {
    /// Top-level code around the bundle statement cannot move to its own
    /// file: a factory depends on it in a way a split would change.
    SurroundingCode,
    /// No module factory could be recovered, or one failed in a way that
    /// rejects the whole container.
    FactoryRecovery,
    /// The bundle carries top-level `import`/`export` declarations whose
    /// public surface cannot be recovered yet.
    EsmDeclarations,
    /// Any other check after the signature matched.
    Other,
}

/// A detector matched `format` and then refused the input for `reason`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SplitRefusal {
    pub format: BundleFormat,
    pub reason: SplitRefusalReason,
}

#[derive(Default)]
struct Capture {
    first: Option<SplitRefusal>,
    /// Reason named by the latest failing check of the open commitment.
    reason: Option<SplitRefusalReason>,
}

thread_local! {
    static CAPTURE: RefCell<Option<Capture>> = const { RefCell::new(None) };
}

/// Runs `detect` and returns the first refusal recorded while it ran.
pub(crate) fn capture<R>(detect: impl FnOnce() -> R) -> (R, Option<SplitRefusal>) {
    let outer = CAPTURE.with(|capture| capture.replace(Some(Capture::default())));
    let result = detect();
    let inner = CAPTURE.with(|capture| capture.replace(outer));
    (result, inner.and_then(|capture| capture.first))
}

/// Marks that a detector matched the signature of `format`. Dropping the
/// result without [`Commitment::accept`] records a refusal.
pub(super) fn commit(format: BundleFormat) -> Commitment {
    with_capture(|capture| capture.reason = None);
    Commitment {
        format,
        accepted: false,
    }
}

/// Names `reason` as the cause of a refusal when `value` is `None`.
pub(super) fn because<T>(value: Option<T>, reason: SplitRefusalReason) -> Option<T> {
    if value.is_none() {
        with_capture(|capture| {
            capture.reason.get_or_insert(reason);
        });
    }
    value
}

pub(super) struct Commitment {
    format: BundleFormat,
    accepted: bool,
}

impl Commitment {
    /// Passes `result` through; a `Some` result is no refusal.
    pub(super) fn accept<T>(mut self, result: Option<T>) -> Option<T> {
        if result.is_some() {
            self.accepted = true;
            with_capture(|capture| capture.reason = None);
        }
        result
    }
}

impl Drop for Commitment {
    fn drop(&mut self) {
        if self.accepted {
            return;
        }
        let format = self.format;
        with_capture(|capture| {
            let reason = capture.reason.take().unwrap_or(SplitRefusalReason::Other);
            capture.first.get_or_insert(SplitRefusal { format, reason });
        });
    }
}

fn with_capture(update: impl FnOnce(&mut Capture)) {
    CAPTURE.with(|capture| {
        if let Some(capture) = capture.borrow_mut().as_mut() {
            update(capture);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refuse(format: BundleFormat, reason: Option<SplitRefusalReason>) -> Option<()> {
        let _commitment = commit(format);
        if let Some(reason) = reason {
            because(None::<()>, reason)?;
        }
        None
    }

    #[test]
    fn records_the_named_reason_of_a_dropped_commitment() {
        let (_, refusal) = capture(|| {
            refuse(
                BundleFormat::Webpack4,
                Some(SplitRefusalReason::SurroundingCode),
            )
        });
        assert_eq!(
            refusal,
            Some(SplitRefusal {
                format: BundleFormat::Webpack4,
                reason: SplitRefusalReason::SurroundingCode,
            })
        );
    }

    #[test]
    fn an_unnamed_refusal_reports_other() {
        let (_, refusal) = capture(|| refuse(BundleFormat::Browserify, None));
        assert_eq!(
            refusal.map(|refusal| refusal.reason),
            Some(SplitRefusalReason::Other)
        );
    }

    #[test]
    fn an_accepted_commitment_records_nothing() {
        let (_, refusal) = capture(|| {
            let commitment = commit(BundleFormat::Webpack5);
            because(None::<()>, SplitRefusalReason::SurroundingCode);
            commitment.accept(Some(()))
        });
        assert_eq!(refusal, None);
    }

    #[test]
    fn a_reason_named_before_the_commitment_does_not_leak_into_it() {
        let (_, refusal) = capture(|| {
            because(None::<()>, SplitRefusalReason::SurroundingCode);
            refuse(BundleFormat::Metro, None)
        });
        assert_eq!(
            refusal.map(|refusal| refusal.reason),
            Some(SplitRefusalReason::Other)
        );
    }

    #[test]
    fn the_first_refusal_wins_and_nested_captures_stay_separate() {
        let (inner, outer) = capture(|| {
            refuse(
                BundleFormat::Webpack4,
                Some(SplitRefusalReason::FactoryRecovery),
            );
            let (_, inner) = capture(|| refuse(BundleFormat::SystemJs, None));
            refuse(BundleFormat::Turbopack, None);
            inner
        });
        assert_eq!(
            inner.map(|refusal| refusal.format),
            Some(BundleFormat::SystemJs)
        );
        assert_eq!(
            outer,
            Some(SplitRefusal {
                format: BundleFormat::Webpack4,
                reason: SplitRefusalReason::FactoryRecovery,
            })
        );
    }

    #[test]
    fn records_nothing_without_an_active_capture() {
        refuse(BundleFormat::Webpack4, None);
        let (_, refusal) = capture(|| ());
        assert_eq!(refusal, None);
    }
}
