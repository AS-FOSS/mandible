#!/usr/bin/env python3
"""A deliberately awkward CLI, for smoke-testing mandible's layout by hand.

Nothing here *does* anything. Every command parses its arguments and exits;
the point is the shape of the `--help` output at each node, which is all
mandible ever reads.

    chmod +x scripts/smoke_cli.py
    mandible ./scripts/smoke_cli.py

Then resize the terminal and watch the right-hand pane. Each subtree targets
one thing that has broken before:

    columns      flag-table alignment — the column must be one number for the
                 whole list at every width, or stack when it cannot be
    unbreakable  tokens wider than the pane; these must break, never vanish
                 into an ellipsis
    prose        long descriptions and summaries, for wrapping and for the
                 tree pane's summary column
    deep         a twelve-level chain, for the breadcrumb, the tree's indent
                 ladder, and lazy extraction depth
    many         sixty flags, for detail-pane vertical scrolling

Written against argparse specifically because it wraps its own output to
`COLUMNS`, which mandible pins to 100 — so the text mandible parses is not
the text you would see running this by hand in a narrow terminal.
"""

from __future__ import annotations

import argparse
import sys

# Long enough that no realistic pane fits it, and with no whitespace to break
# at — the case that used to render as a column of ellipses.
LONG_URL = (
    "https://registry.example.com/v2/organisation/team/project/artifacts/"
    "sha256:3f786850e387550fdab836ed7e6dc881de23001b4b4f4b4f4b4f4b4f4b4f4b4f/manifest.json"
)

LONG_DEFAULT = (
    "/opt/vendor/product/etc/configuration/profiles/production/regional/"
    "eu-west-1/overrides/defaults.yaml"
)

PARAGRAPH = (
    "Reconciles the declared state of every managed resource against what the "
    "control plane currently reports, computes the minimal ordered set of "
    "operations that would bring the two into agreement, and either applies "
    "that plan or writes it out for review. Resources that cannot be "
    "reconciled without destroying data are never touched implicitly; they are "
    "listed, and the command exits non-zero so that a pipeline stops rather "
    "than proceeding on a partial result."
)

DEEP_CHAIN = [
    ("infrastructure", "Manage infrastructure across every configured region"),
    ("provisioning", "Create, update and retire provisioned resources"),
    ("kubernetes", "Kubernetes-specific provisioning operations"),
    ("clusters", "Manage clusters within the selected control plane"),
    ("nodepools", "Manage node pools attached to a cluster"),
    ("autoscaling", "Autoscaling behaviour for a node pool"),
    ("policies", "Named autoscaling policies and their bindings"),
    ("thresholds", "Scale-up and scale-down thresholds for a policy"),
    ("overrides", "Per-environment threshold overrides"),
    ("regional", "Overrides scoped to a single region"),
    ("failover", "Behaviour when the primary region is unavailable"),
    ("configure", "Write the resolved failover configuration"),
]


def add_commands(parser: argparse.ArgumentParser) -> argparse._SubParsersAction:
    """Attach a subcommand group that argparse will actually list in --help.

    Two traps here, both of which make this script a test of nothing:

    - Passing `metavar` replaces the choice list with a placeholder and hides
      every subcommand from the help text.
    - Passing `title=` moves the block out of `positional arguments:` and
      under a heading of your choosing — which mandible does not currently
      read as a command list, so the whole tree collapses to a single node.
      That is a mandible bug rather than an argparse one, but until it is
      fixed, styling this heading silently disables the fixture.
    """
    return parser.add_subparsers(dest="command")


def build_columns(sub: argparse._SubParsersAction) -> None:
    """Flag-table alignment: every description must start at one column."""
    group = sub.add_parser(
        "columns",
        help="Flag-table alignment cases",
        description="Each subcommand here is a different shape of flag list.",
    )
    inner = add_commands(group)

    mixed = inner.add_parser(
        "mixed",
        help="Short and long spellings, some taking values",
        description="The common case, and the one that rendered ragged: a "
        "list where some rows are much wider than others.",
    )
    mixed.add_argument("-a", "--all-tags", action="store_true", help="Include every tag")
    mixed.add_argument("-q", "--quiet", action="store_true", help="Suppress verbose output")
    mixed.add_argument("--platform", metavar="string", help="Set platform if server is multi-platform capable")
    mixed.add_argument("-l", "--log-level", metavar="string", help="Set the logging level")
    mixed.add_argument("-c", "--context", metavar="string", help="Name of the context to use")
    mixed.add_argument("--tls", action="store_true", help="Use TLS; implied by --tlsverify")

    outlier = inner.add_parser(
        "outlier",
        help="One enormous spelling among short ones",
        description="The long row must hang its description onto the next "
        "line rather than dragging the column right for everyone else.",
    )
    outlier.add_argument("-v", action="store_true", help="Verbose")
    outlier.add_argument("-n", action="store_true", help="Dry run")
    outlier.add_argument(
        "--an-extremely-long-option-name-that-nobody-would-ever-type-by-hand",
        action="store_true",
        help="Exists purely to be too wide for the column",
    )

    bare = inner.add_parser(
        "bare",
        help="No flag takes a value (the value column should collapse)",
        description="With nothing to put in it, the value column must "
        "disappear rather than leave a blank strip down the pane.",
    )
    for name, helptext in [
        ("--force", "Proceed without confirmation"),
        ("--dry-run", "Show what would happen and stop"),
        ("--recursive", "Descend into child resources"),
        ("--no-verify", "Skip signature verification"),
    ]:
        bare.add_argument(name, action="store_true", help=helptext)

    valued = inner.add_parser(
        "valued",
        help="Every flag takes a long value placeholder",
        description="Pushes the description column as far right as the "
        "layout will allow before it has to stack.",
    )
    valued.add_argument("--source-repository", metavar="REPOSITORY_URL", help="Where to read from")
    valued.add_argument("--destination-registry", metavar="REGISTRY_HOSTNAME", help="Where to write to")
    valued.add_argument("--credentials-file", metavar="PATH_TO_CREDENTIALS", help="Credentials to authenticate with")


def build_unbreakable(sub: argparse._SubParsersAction) -> None:
    """Tokens with no whitespace to wrap at."""
    group = sub.add_parser(
        "unbreakable",
        help="Tokens wider than any pane",
        description="These must break across lines. Replacing one with an "
        "ellipsis loses the whole token, which is the defect.",
    )
    inner = add_commands(group)

    url = inner.add_parser("url", help="A description ending in a very long URL")
    url.add_argument("--manifest", metavar="URL", help=f"Fetch the manifest from {LONG_URL}")

    enum = inner.add_parser("enum", help="A long list of permitted values")
    enum.add_argument(
        "--output-format",
        choices=[
            "json", "json-pretty", "yaml", "toml", "ndjson", "csv",
            "tsv", "table", "table-wide", "template", "go-template",
            "jsonpath", "custom-columns", "name", "wide",
        ],
        help="Output format",
    )

    default = inner.add_parser("default", help="A very long default value")
    default.add_argument("--config", metavar="PATH", default=LONG_DEFAULT, help="Configuration file (default: %(default)s)")


def build_prose(sub: argparse._SubParsersAction) -> None:
    """Long prose, for the detail pane and the tree's summary column."""
    group = sub.add_parser(
        "prose",
        help="A command whose one-line summary is itself far too long to fit in any tree pane at any width",
        description=PARAGRAPH,
    )
    group.add_argument("--plan-only", action="store_true", help="Write the plan out for review instead of applying it")
    group.add_argument(
        "--reconciliation-strategy",
        metavar="STRATEGY",
        help="How to resolve a resource whose declared state and reported "
        "state disagree in ways that cannot be reconciled without "
        "destroying data that the control plane has no copy of",
    )


def build_many(sub: argparse._SubParsersAction) -> None:
    """Enough flags to need scrolling in the detail pane."""
    group = sub.add_parser("many", help="Sixty flags, for detail-pane scrolling")
    for i in range(60):
        help_text = (
            f"The {i:02d}th option, described at a length that will wrap in a "
            "narrow pane but not a wide one"
        )
        # `store_true` rejects `metavar` outright rather than ignoring it, so
        # the two shapes cannot share one call.
        if i % 3 == 0:
            group.add_argument(f"--option-number-{i:02d}", metavar="VALUE", help=help_text)
        else:
            group.add_argument(f"--option-number-{i:02d}", action="store_true", help=help_text)


def build_deep(sub: argparse._SubParsersAction) -> None:
    """A twelve-level chain: breadcrumb, indent ladder, extraction depth."""
    parser = sub.add_parser(DEEP_CHAIN[0][0], help=DEEP_CHAIN[0][1])
    for name, helptext in DEEP_CHAIN[1:]:
        inner = add_commands(parser)
        parser = inner.add_parser(name, help=helptext)
        parser.add_argument("--region", metavar="REGION", help="Region this level applies to")
    # The leaf carries real content, so the end of the chain is worth
    # reaching rather than being an empty node.
    parser.add_argument("--primary", metavar="REGION", help="Region to prefer while it is healthy")
    parser.add_argument("--secondary", metavar="REGION", help="Region to fail over to")
    parser.add_argument("--drain-timeout", metavar="DURATION", default="5m", help="How long to wait for connections to drain before cutting over (default: %(default)s)")


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(
        description="A CLI that exists to be rendered, not run. Every "
        "subtree stresses one part of a TUI's layout.",
    )
    parser.add_argument("--verbose", "-v", action="count", default=0, help="Increase verbosity (repeatable)")
    parser.add_argument("--config", metavar="FILE", help="Read configuration from FILE")

    sub = add_commands(parser)
    build_columns(sub)
    build_unbreakable(sub)
    build_prose(sub)
    build_many(sub)
    build_deep(sub)

    parser.parse_args(argv)
    # Inert by design: mandible executes this, so it must never do anything
    # beyond printing its own help.
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
