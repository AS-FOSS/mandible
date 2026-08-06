#!/usr/bin/env python3
import click

@click.group()
@click.option("--verbose", is_flag=True, help="Be verbose")
def cli(verbose):
    """Manage widgets."""

@cli.command()
@click.argument("name")
def init(name):
    """Initialize a new widget."""

@cli.command()
def build():
    """Build the widget."""

if __name__ == "__main__":
    cli()
