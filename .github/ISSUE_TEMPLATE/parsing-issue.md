---
name: A tool renders wrong
about: mandible shows the wrong flags, missing commands, or invented ones
title: '<tool> renders incorrectly'
labels: parsing
---

<!--
The most useful thing you can include is the output of --doctor, because it
names the *framework* mandible thinks generated the help text. That turns
"mandible is wrong about tool X" into "the argparse grammar mishandles Y" —
a general, fixable bug instead of a per-tool complaint.

mandible never fixes tools one at a time (see the rule in the README), so a
report that identifies the framework is a report that can actually be acted
on.
-->

**Tool and version**

```console
$ <tool> --version
```

**What `--doctor` says**

```console
$ mandible --doctor <tool>
```

**What the tool actually prints**

```console
$ <tool> --help
```

**What mandible shows instead**

<!-- Paste the pane, or describe it. Screenshots are fine. -->

**What you expected**
