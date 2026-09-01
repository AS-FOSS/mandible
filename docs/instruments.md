# xtask instruments

`xtask` carries six measuring instruments, each a top-level subcommand.
Each paragraph below names the instrument, the question it answers, and
what it would take to retire it. Order follows `xtask/src/main.rs`'s
`Command` enum.

## Coverage (`xtask coverage`)

Coverage runs the full extraction pipeline against every executable on
`PATH` and scores the aggregate: how many tools produced structure, and
what fraction of a document's text a parse actually accounted for. It
answers "did this change move the fleet-wide numbers in the wrong
direction". It measures agreement with the parser's own prior output, not
with ground truth, so it retires only once a ground-truth instrument can
run at full-`PATH` scale instead of on an 80-tool sample. `xtask audit` is
that instrument today, and it cannot yet reach that scale because each
tool needs a human minute.

## Corpus (`xtask corpus`)

Corpus replays every fixture under `corpus/<tool>/<version>/` against the
real pipeline from frozen bytes and fails when a parse regresses. It
answers "did a change break a tool that was already fixed". It is the
project's permanent regression net, built to grow rather than shrink, so
the instrument itself has no retirement condition. A single fixture
retires only when its capture is proven wrong, per `corpus/README.md`'s
own lifecycle rules, never because the tool it names became inconvenient.

## SweepDiff (`xtask sweep-diff`)

SweepDiff compares two rendered coverage scoreboards and reports which
tools gained or lost flags, and which changed parse status, without
netting gains against losses. It answers "which specific tools did this
change touch", the question the fleet-wide aggregate cannot answer
because a four-tool regression moves it by hundredths of a percent. It
ships as a non-blocking report by maintainer decision. It retires as a
standalone command once its check is wired into `coverage --check` as a
hard gate after a burn-in period; a human would no longer need to run it
by hand before and after a change.

## Audit (`xtask audit`)

Audit draws a bounded, random, human-reviewed sample of real tools and
compares each one's raw `--help` text against its parsed tree. It answers
"how accurate is the parser against ground truth", the number no other
instrument here measures, since every other one compares the parser
against itself. Each reviewed tool also becomes a corpus fixture, so the
review effort is never spent twice. It retires once every defect family
the audit has ever found has a calibrated detector (see Detector) with a
fleet-wide number trusted without further human sampling.

## Detector (`xtask detector`)

Detector calibrates a fleet-wide defect-family check against the audit's
own labelled tools: it must fire on the known-bad tools and stay silent
on the known-good ones before its fleet-wide count may be quoted. It
answers "can this detector's number be trusted yet". A single detector
retires once its defect family stops occurring in the calibration set at
all, because the underlying grammar bug is fixed for every labelled tool
that carried it. The harness itself retires once no detector is left that
still needs recalibration against human labels.

## Residue (`xtask residue`)

Residue ranks captured `--help` documents by how much structurally
plausible text a parse left unaccounted for, the omission complement of
the existence check. It answers "what should a human read next to find
the next missed shape". It is explicitly barred from ever becoming a
gate or a quoted number, enforced by a test that fails the build if it
ever gets wired into one. It retires for a given tool once every omission
shape it surfaces there is already covered by a calibrated rule or
detector, so the ranking stops finding anything new to read.
