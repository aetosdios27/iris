# V1 Release Checklist

## Scope Lock

- V1 scope is frozen
- V1 non-goals are explicitly documented
- release notes draft exists

## Build and Packaging

- release build completes cleanly
- install instructions are tested on a clean machine
- packaging format is selected
- binary/package naming is finalized
- app metadata is finalized

## Runtime Modes

- active render path is logged at startup
- zero-copy mode is distinguishable from fallback mode
- unsupported fast-path environments fail gracefully
- software fallback is verified

## Core Viewer Workflows

- open single file works
- open directory works
- drag and drop file works
- drag and drop directory works
- thumbnail strip populates correctly
- keyboard navigation works
- zoom works
- pan works
- rotation works
- metadata panel works
- window/config persistence works

## Stability

- resize path no longer stalls
- no known descriptor pool exhaustion path in realistic browsing
- no known FD leak in steady-state use
- animated image handling is bounded
- no obvious crash path on unsupported GPUs/compositors

## Compatibility

- GNOME Wayland validated
- KDE Wayland validated
- at least one non-fast-path Linux environment validated
- support matrix is published
- known limitations are published

## Performance

- benchmark corpus is fixed
- benchmark methodology is published
- benchmark runs are completed
- claims are based on measured numbers

## Documentation

- README reflects current product reality
- architecture docs do not contradict current code
- support matrix exists
- benchmark plan exists
- release criteria exists
- execution plan exists

## Final Go/No-Go Questions

- would you hand this build to a Linux user who is not inside the project?
- if zero-copy fails on their setup, does the app remain usable?
- are the known limitations honest enough to survive public scrutiny?

If any answer is no, do not tag V1.
