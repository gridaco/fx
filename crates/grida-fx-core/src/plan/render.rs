//! `grida-fx plan` text, byte for byte the layout of the engine FX comes from (stage-gen's
//! gnode): a 10-column label field, `·` U+00B7, `–` U+2013, `≤` U+2264, `⚠` U+26A0, money
//! `$x.xx`, `$lo – $hi` unless equal, `1 steps`, `provider calls` always plural,
//! `phase`/`phases`. No trailing newline.
//!
//! ```text
//! {id}  ·  {k} phase|phases
//! phase {n}   {steps} steps   {calls}   {money}[   then: {pending, ", "}]
//! phase {n}   {pending, ", "}   ≤ ${high}   priced exactly when its list exists
//! cached    {cached} of {known} known steps
//! estimate  {money}[   ceiling ${c}][   ⚠ the worst case exceeds the ceiling; the run stops before crossing it]
//! note      {warning}
//! refused   {where}: {message}
//! ```
//!
//! - A phase with no steps and pending repeats prints the second `phase` form; every other phase
//!   the first. `{calls}` is `{lo} provider calls`, or `{lo}–{hi} provider calls` (no spaces
//!   around the dash) when the counts differ.
//! - `{money}` is `$lo` when both ends are equal, else `$lo – $hi` (spaces around the dash).
//! - The ceiling warning appears when a ceiling exists and the estimate's high end exceeds it.
//! - `note` lines are the plan's warnings, `refused` lines its problems, both in order.
//!
//! Amounts are shown with two decimals the way the predecessor's `f"${x:.2f}"` showed its
//! floats: the amount as the nearest double, correctly rounded (Rust's `{:.2}` rounds the same
//! way), so `$0.005` shows `$0.01` and `$0.125` shows `$0.12`.

use super::{Estimate, PhaseSummary, Plan};
use crate::error::Problem;
use crate::money::Usd;
use crate::project::Planner;

/// U+2013, between call counts and between money amounts.
const DASH: char = '\u{2013}';
/// U+00B7, between the workflow id and the phase count.
const DOT: char = '\u{b7}';
/// U+2264, before a pending phase's worst case.
const AT_MOST: char = '\u{2264}';
/// U+26A0, before the ceiling warning.
const WARNING: char = '\u{26a0}';

/// The plan as text.
pub fn render(plan: &Plan, planner: &Planner) -> String {
    let phases = plan.phases();
    let warnings = plan.warnings(planner);
    lay_out(&Summary {
        workflow: &planner.workflow.workflow.id,
        phases: &phases,
        cached: plan.cached.len(),
        known: plan.known(),
        estimate: plan.estimate(),
        ceiling: plan.ceiling,
        warnings: &warnings,
        problems: &plan.problems,
    })
}

/// Everything the text shows, computed.
struct Summary<'a> {
    workflow: &'a str,
    phases: &'a [PhaseSummary],
    cached: usize,
    known: usize,
    estimate: Estimate,
    ceiling: Option<Usd>,
    warnings: &'a [String],
    problems: &'a [Problem],
}

fn lay_out(summary: &Summary<'_>) -> String {
    let count = summary.phases.len();
    let mut lines = vec![format!(
        "{}  {DOT}  {count} phase{}",
        summary.workflow,
        if count == 1 { "" } else { "s" }
    )];
    for phase in summary.phases {
        lines.push(phase_line(phase));
    }
    lines.push(format!(
        "cached    {} of {} known steps",
        summary.cached, summary.known
    ));
    let mut estimate = format!(
        "estimate  {}",
        money_range(summary.estimate.low, summary.estimate.high)
    );
    if let Some(ceiling) = summary.ceiling {
        estimate.push_str(&format!("   ceiling {}", dollars(ceiling)));
        if summary.estimate.high > ceiling {
            estimate.push_str(&format!(
                "   {WARNING} the worst case exceeds the ceiling; the run stops before crossing it"
            ));
        }
    }
    lines.push(estimate);
    for warning in summary.warnings {
        lines.push(format!("note      {warning}"));
    }
    for problem in summary.problems {
        lines.push(format!("refused   {problem}"));
    }
    lines.join("\n")
}

fn phase_line(phase: &PhaseSummary) -> String {
    if phase.steps == 0 && !phase.pending.is_empty() {
        return format!(
            "phase {}   {}   {AT_MOST} {}   priced exactly when its list exists",
            phase.phase,
            phase.pending.join(", "),
            dollars(phase.high)
        );
    }
    let calls = if phase.calls_low == phase.calls_high {
        format!("{} provider calls", phase.calls_low)
    } else {
        format!(
            "{}{DASH}{} provider calls",
            phase.calls_low, phase.calls_high
        )
    };
    let then = if phase.pending.is_empty() {
        String::new()
    } else {
        format!("   then: {}", phase.pending.join(", "))
    };
    format!(
        "phase {}   {} steps   {calls}   {}{then}",
        phase.phase,
        phase.steps,
        money_range(phase.low, phase.high)
    )
}

/// `$lo`, or `$lo – $hi` when the ends differ. Equality is decided on the amounts, before
/// rounding: `$0.001` to `$0.002` shows `$0.00 – $0.00`.
fn money_range(low: Usd, high: Usd) -> String {
    if low == high {
        dollars(low)
    } else {
        format!("{} {DASH} {}", dollars(low), dollars(high))
    }
}

/// `$x.xx` (module doc).
fn dollars(amount: Usd) -> String {
    format!("${:.2}", amount.0 as f64 / 1_000_000.0)
}

#[cfg(test)]
mod tests {
    //! The layout is tested on computed summaries, reproducing the plans the predecessor printed
    //! for the conformance cases (`refusals`, `tiered-price`, `judge-regenerate`, `phase`,
    //! `linear`). The `integrated_` tests render whole hand-built plans, so they also hold
    //! [`Plan`]'s pricing to the same text; they need the planning module's summaries.

    use super::*;

    fn phase(
        number: u32,
        steps: usize,
        calls: (u64, u64),
        money: (i64, i64),
        pending: &[&str],
    ) -> PhaseSummary {
        PhaseSummary {
            phase: number,
            steps,
            calls_low: calls.0,
            calls_high: calls.1,
            low: Usd(money.0),
            high: Usd(money.1),
            pending: pending.iter().map(|p| p.to_string()).collect(),
        }
    }

    fn summary<'a>(
        phases: &'a [PhaseSummary],
        known: usize,
        estimate: (i64, i64),
        ceiling: Option<i64>,
        warnings: &'a [String],
        problems: &'a [Problem],
    ) -> Summary<'a> {
        Summary {
            workflow: "case",
            phases,
            cached: 0,
            known,
            estimate: Estimate {
                low: Usd(estimate.0),
                high: Usd(estimate.1),
            },
            ceiling: ceiling.map(Usd),
            warnings,
            problems,
        }
    }

    #[test]
    fn refusals() {
        let phases = [
            phase(1, 4, (3, 3), (13_000, 46_000), &[]),
            phase(2, 0, (0, 0), (0, 0), &["unbounded (up to 1)"]),
        ];
        let problems = [
            Problem::new("workflow.assert[0]", "at most one name"),
            Problem::new("draw.requires", "img-a@acme does not support mask"),
            Problem::new(
                "review.independent_of",
                "shares the model llm-a with write; route one of them to a different model",
            ),
            Problem::new(
                "lost.route",
                "no route img-z@nowhere serves image.generate (known routes: img-a@acme)",
            ),
            Problem::new(
                "unbounded",
                "the list comes from a step, so the plan cannot count it: add max: (the most items this repeat may run)",
            ),
        ];
        let text = lay_out(&summary(&phases, 4, (13_000, 46_000), None, &[], &problems));
        assert_eq!(
            text,
            "case  ·  2 phases\n\
             phase 1   4 steps   3 provider calls   $0.01 – $0.05\n\
             phase 2   unbounded (up to 1)   ≤ $0.00   priced exactly when its list exists\n\
             cached    0 of 4 known steps\n\
             estimate  $0.01 – $0.05\n\
             refused   workflow.assert[0]: at most one name\n\
             refused   draw.requires: img-a@acme does not support mask\n\
             refused   review.independent_of: shares the model llm-a with write; route one of them to a different model\n\
             refused   lost.route: no route img-z@nowhere serves image.generate (known routes: img-a@acme)\n\
             refused   unbounded: the list comes from a step, so the plan cannot count it: add max: (the most items this repeat may run)"
        );
    }

    #[test]
    fn tiered_price() {
        let phases = [phase(1, 3, (3, 3), (2_490_000, 6_862_500), &[])];
        let text = lay_out(&summary(&phases, 3, (2_490_000, 6_862_500), None, &[], &[]));
        assert_eq!(
            text,
            "case  ·  1 phase\n\
             phase 1   3 steps   3 provider calls   $2.49 – $6.86\n\
             cached    0 of 3 known steps\n\
             estimate  $2.49 – $6.86"
        );
    }

    #[test]
    fn judge_regenerate_with_a_ceiling() {
        let phases = [phase(1, 7, (1, 3), (10_000, 120_000), &[])];
        let text = lay_out(&summary(
            &phases,
            3,
            (10_000, 120_000),
            Some(1_000_000),
            &[],
            &[],
        ));
        assert_eq!(
            text,
            "case  ·  1 phase\n\
             phase 1   7 steps   1–3 provider calls   $0.01 – $0.12\n\
             cached    0 of 3 known steps\n\
             estimate  $0.01 – $0.12   ceiling $1.00"
        );
    }

    #[test]
    fn phase_priced_when_its_list_exists() {
        let phases = [
            phase(1, 1, (0, 0), (0, 0), &[]),
            phase(2, 0, (0, 0), (0, 240_000), &["draw (up to 6)"]),
        ];
        let text = lay_out(&summary(
            &phases,
            1,
            (0, 240_000),
            Some(1_000_000),
            &[],
            &[],
        ));
        assert_eq!(
            text,
            "case  ·  2 phases\n\
             phase 1   1 steps   0 provider calls   $0.00\n\
             phase 2   draw (up to 6)   ≤ $0.24   priced exactly when its list exists\n\
             cached    0 of 1 known steps\n\
             estimate  $0.00 – $0.24   ceiling $1.00"
        );
    }

    #[test]
    fn linear_over_its_ceiling() {
        let phases = [phase(1, 2, (1, 1), (10_000, 40_000), &[])];
        let text = lay_out(&summary(
            &phases,
            1,
            (10_000, 40_000),
            Some(10_000),
            &[],
            &[],
        ));
        assert_eq!(
            text,
            "case  ·  1 phase\n\
             phase 1   2 steps   1 provider calls   $0.01 – $0.04\n\
             cached    0 of 1 known steps\n\
             estimate  $0.01 – $0.04   ceiling $0.01   ⚠ the worst case exceeds the ceiling; the run stops before crossing it"
        );
    }

    #[test]
    fn a_ceiling_met_exactly_gives_no_warning() {
        let phases = [phase(1, 2, (1, 1), (10_000, 40_000), &[])];
        let text = lay_out(&summary(
            &phases,
            1,
            (10_000, 40_000),
            Some(40_000),
            &[],
            &[],
        ));
        assert!(text.ends_with("estimate  $0.01 – $0.04   ceiling $0.04"));
    }

    #[test]
    fn notes_come_before_refusals() {
        let phases = [phase(1, 2, (0, 0), (0, 0), &[])];
        let warnings = vec![
            "the takes file names counted, which no step is any more; move it with grida-fx takes mv case \"counted\" <new path>".to_string(),
        ];
        let problems = [Problem::new(
            "draw.route",
            "no route img-a@acme serves image.generate (known routes: none)",
        )];
        let mut text = lay_out(&summary(&phases, 1, (0, 0), None, &warnings, &problems));
        text.push('\n');
        assert_eq!(
            text,
            "case  ·  1 phase\n\
             phase 1   2 steps   0 provider calls   $0.00\n\
             cached    0 of 1 known steps\n\
             estimate  $0.00\n\
             note      the takes file names counted, which no step is any more; move it with grida-fx takes mv case \"counted\" <new path>\n\
             refused   draw.route: no route img-a@acme serves image.generate (known routes: none)\n"
        );
    }

    #[test]
    fn a_phase_with_steps_and_pending_repeats() {
        let line = phase_line(&phase(
            1,
            2,
            (2, 4),
            (20_000, 80_000),
            &["a (up to 2)", "b (up to 3)"],
        ));
        assert_eq!(
            line,
            "phase 1   2 steps   2–4 provider calls   $0.02 – $0.08   then: a (up to 2), b (up to 3)"
        );
    }

    #[test]
    fn money_shows_two_decimals_as_the_predecessor_did() {
        assert_eq!(dollars(Usd(0)), "$0.00");
        assert_eq!(dollars(Usd(6_862_500)), "$6.86");
        assert_eq!(dollars(Usd(5_000)), "$0.01");
        assert_eq!(dollars(Usd(15_000)), "$0.01");
        assert_eq!(dollars(Usd(125_000)), "$0.12");
        assert_eq!(dollars(Usd(2_675_000)), "$2.67");
        assert_eq!(dollars(Usd(1_234_000_000)), "$1234.00");
        assert_eq!(money_range(Usd(1_000), Usd(2_000)), "$0.00 – $0.00");
        assert_eq!(money_range(Usd(40_000), Usd(40_000)), "$0.04");
    }

    mod integrated {
        //! Whole plans rendered: the instances, prices and pending repeats of the captured cases,
        //! built by hand.

        use super::super::render;
        use crate::plan::output::test_support::*;

        #[test]
        fn integrated_refusals() {
            let (plan, planner) = refusals_plan();
            assert_eq!(
                render(&plan, &planner),
                "case  ·  2 phases\n\
                 phase 1   4 steps   3 provider calls   $0.01 – $0.05\n\
                 phase 2   unbounded (up to 1)   ≤ $0.00   priced exactly when its list exists\n\
                 cached    0 of 4 known steps\n\
                 estimate  $0.01 – $0.05\n\
                 refused   workflow.assert[0]: at most one name\n\
                 refused   draw.requires: img-a@acme does not support mask\n\
                 refused   review.independent_of: shares the model llm-a with write; route one of them to a different model\n\
                 refused   lost.route: no route img-z@nowhere serves image.generate (known routes: img-a@acme)\n\
                 refused   unbounded: the list comes from a step, so the plan cannot count it: add max: (the most items this repeat may run)"
            );
        }

        #[test]
        fn integrated_tiered_price() {
            let (plan, planner) = tiered_plan();
            assert_eq!(
                render(&plan, &planner),
                "case  ·  1 phase\n\
                 phase 1   3 steps   3 provider calls   $2.49 – $6.86\n\
                 cached    0 of 3 known steps\n\
                 estimate  $2.49 – $6.86"
            );
        }

        #[test]
        fn integrated_judge_regenerate() {
            let (plan, planner) = judge_regenerate_plan();
            assert_eq!(
                render(&plan, &planner),
                "case  ·  1 phase\n\
                 phase 1   7 steps   1–3 provider calls   $0.01 – $0.12\n\
                 cached    0 of 3 known steps\n\
                 estimate  $0.01 – $0.12   ceiling $1.00"
            );
        }

        #[test]
        fn integrated_phase() {
            let (plan, planner) = phase_plan();
            assert_eq!(
                render(&plan, &planner),
                "case  ·  2 phases\n\
                 phase 1   1 steps   0 provider calls   $0.00\n\
                 phase 2   draw (up to 6)   ≤ $0.24   priced exactly when its list exists\n\
                 cached    0 of 1 known steps\n\
                 estimate  $0.00 – $0.24   ceiling $1.00"
            );
        }
    }
}
