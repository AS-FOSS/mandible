# How accuracy is measured

The accuracy chart in the README answers one question: if you open a tool mandible has never
seen, how likely is it that mandible reads it fully correctly? This page explains where each
number comes from, so anyone can check it or reproduce it.

## What counts as correct

A tool is **correct** when every one of its flags, subcommands, positionals and flag values is
read right, on every page. One misread flag makes the whole tool wrong. Descriptions and layout
are not measured.

Each tool gets one of four verdicts:

| Verdict | Meaning |
|---|---|
| correct | everything measured is right |
| incomplete | something is missing, nothing is wrong |
| wrong | something is read wrongly |
| skip | the reviewer could not judge it (left out of every figure, but listed) |

Only `correct` counts towards accuracy. `incomplete` and `wrong` both count against it.

## How a batch is drawn and judged

1. **The draw is fixed in advance.** The 2,299 tools on the reference machine's `PATH` in August
   2026 were shuffled once and the order committed to the repo (`audit/queue.toml`). A batch
   takes the next tools in that order, so nobody can redraw a sample that looks bad. A tool that
   already has a verdict or a test fixture, or is no longer installed, is passed over, and the
   batch file records each one with its reason: the figure is about tools mandible was not tuned
   for. Freezing a new queue starts a new population.
2. **A person judges each tool by eye,** in the interface itself, with `mandible --review
   <seed>`. The tool opens exactly as `mandible <tool>` would, `t` shows the tool's own help
   text, and the verdict is saved straight away. No scripts and no AI form verdicts.
3. **The batch records the release it was judged with** (`mandible_version` in the batch file).

A batch is about 50 tools. Two batches judged with the same release pool into one figure.

## Scoring older releases

A batch also scores every release that came out **before** it was drawn. Releases made after a
batch are never scored on it: by then its failures have been seen and fixed, so it would flatter
them.

Each of those releases is run on the batch's tools, on the same machine and at the same time as
the judged release (`cargo xtask audit replay`). Each run keeps only what is measured (flags,
subcommands, positionals and values) and compares it with what the judged release read:

| Older release reads the tool... | Judged verdict | Older release counts as |
|---|---|---|
| the same as the judged release | any | the same verdict |
| differently | correct | wrong |
| differently | incomplete or wrong | correct if it matches the tool's truth file, wrong if not |
| differently, and there is no truth file yet | incomplete or wrong | awaiting review |

A **truth file** (`audit/<seed>/truth/<tool>.json`) records what a tool really contains. It is
written once a fix for that tool lands and a person has confirmed the fixed reading by eye.
Tools awaiting review are left out of the figure and listed under the chart.

So every new batch adds a point for the newest release and more data for every older one. Older
releases have more batches behind them and narrower ranges.

## The range around each point

A sample of 50 tools cannot give an exact rate. Each point on the chart carries a bar showing
the range the true rate plausibly falls in, at 95 % confidence, using the Wilson score interval.
For $k$ correct tools out of $n$ judged, with $\hat p = k/n$ and $z = 1.96$:

$$
\frac{\hat p + \frac{z^2}{2n} \pm z\sqrt{\frac{\hat p(1-\hat p)}{n} + \frac{z^2}{4n^2}}}{1 + \frac{z^2}{n}}
$$

How wide that range is, for a tool rate around 60 %:

| Tools judged | Range |
|---|---|
| 30 | about ±17 points |
| 50 | about ±13 points |
| 100 | about ±10 points |
| 200 | about ±7 points |

Two points whose bars overlap a lot are not reliably different.

## New and known failures

Every failure is tagged once it has been looked at:

- **known**: its help-text layout is already described in [`shapes.md`](shapes.md), and something
  in handling it is wrong;
- **new**: a layout mandible has not met before. It gets an entry in `shapes.md` when it is fixed.

The second chart counts both per batch. As fresh batches turn up fewer new layouts, mandible is
running out of help-text layouts it has not seen.

## The goal

The goal line is at **90 %** fully correct. Some help text cannot be read reliably by any rules
at all, so the ceiling is below 100 %. The goal may be revised once the curve shows where it
flattens.

## Adding a batch

```console
$ cargo xtask audit sample --seed <seed> --sample 50         # draw the next 50 tools
$ mandible --review <seed>                                   # judge them by eye
$ cargo xtask audit replay --seed <seed> --per-minor         # run the older releases
$ cargo xtask audit tag --seed <seed> --tool <tool> --kind new   # tag each failure
$ cargo xtask audit history                                  # score every release
$ cargo xtask audit chart                                    # redraw the charts
```

Run the replay soon after judging, before the tools on the machine are upgraded: the history
step refuses a batch whose tools changed between runs.

## Audits from other machines

`cargo xtask audit contribute` lets anyone audit the tools on their own machine, with the same
verdicts and the same pass-over rule. Those batches come from a different set of tools, so they
stay out of the main chart, where they would move the line for reasons that have nothing to do
with the parser. Their reports sit under `audit/submissions/`.

## Earlier audits

Audits from before this method (seeds 2 to 7, August and September 2026) did not record the
release they were judged with, and their runs cannot be replayed. They stay in `audit/` as
records but are not part of the chart.
