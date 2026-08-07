---
name: Add support for a CLI framework
about: A generator mandible does not recognize yet
title: 'Support <framework>'
labels: framework
---

<!--
Support is per *framework*, never per tool — one grammar covers every CLI
that framework ever generated. Adding one is one `match` arm in
mandible-extract/src/help_text/profile.rs plus one fingerprint in
mandible-extract/src/framework/.
-->

**Framework** (name, language, link)

**How to identify it from the artifact**
<!-- A string embedded in compiled binaries, or the import line in scripts.
     This is the reliable signal; help-text headings are the fallback. -->

**How to identify it from help text**
<!-- A distinctive marker string, e.g. argparse's "show this help message
     and exit". -->

**Two or three real tools that use it**

**A representative `--help` output**

```
```
