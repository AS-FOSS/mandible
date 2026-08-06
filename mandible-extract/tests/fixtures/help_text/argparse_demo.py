#!/usr/bin/env python3
import argparse

def main():
    parser = argparse.ArgumentParser(prog="widget", description="Manage widgets.")
    parser.add_argument("-v", "--verbose", action="store_true", help="be verbose")
    parser.add_argument("--config", metavar="FILE", help="path to config file")
    sub = parser.add_subparsers(dest="command")
    init = sub.add_parser("init", help="Initialize a new widget")
    init.add_argument("name", help="widget name")
    build = sub.add_parser("build", help="Build the widget")
    run = sub.add_parser("run", help="Run the widget")
    parser.parse_args()

if __name__ == "__main__":
    main()
