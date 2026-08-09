#!/usr/bin/env python3
"""Same shape as argparse_demo.py, but with add_subparsers(title=...) set.

Exists to reproduce Bug A: a custom subparsers title (`title="commands"`)
renders as `commands:` instead of argparse's own default `positional
arguments:`. `scan_argparse_subparsers`'s gate used to require the heading
text to contain "positional arguments" *in addition to* the structural
`{choice,...}` pseudo-entry evidence, so a titled block never reached the
dedicated scan at all and fell through to the general bare-block engine,
which reads the `{init,build,run}` pseudo-entry as one entry and the three
real subcommands as its wrapped continuation lines — losing all of them but
one phantom node.
"""
import argparse


def main():
    parser = argparse.ArgumentParser(prog="widget", description="Manage widgets.")
    parser.add_argument("-v", "--verbose", action="store_true", help="be verbose")
    parser.add_argument("--config", metavar="FILE", help="path to config file")
    sub = parser.add_subparsers(title="commands", dest="command")
    init = sub.add_parser("init", help="Initialize a new widget")
    init.add_argument("name", help="widget name")
    build = sub.add_parser("build", help="Build the widget")
    run = sub.add_parser("run", help="Run the widget")
    parser.parse_args()


if __name__ == "__main__":
    main()
