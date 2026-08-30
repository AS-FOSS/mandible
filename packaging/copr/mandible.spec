# RPM spec for mandible, built from the tagged source archive plus a
# vendored copy of every crate the build needs.
#
# Why vendored crates, and why as a Source rather than fetched:
#
# A mock/COPR build has no network. Something therefore has to put the
# dependency tree inside the build root, and there were two honest options.
# COPR's custom source method would run a script (network allowed) that
# generates the sources at submit time; that keeps the release assets small
# but makes the SRPM unreproducible outside COPR, and it cannot be built or
# debugged locally, in koji, or by a packager with an rpmbuild and no COPR
# account. Shipping `cargo vendor` output as Source1 instead makes the SRPM
# self-contained: it builds offline anywhere, its inputs are two files with
# published checksums, and `rpmbuild --rebuild` reproduces exactly what
# COPR built. The cost is a ~34 MB release asset per version, which is the
# price of the SRPM being a complete description of the build.
#
# release.yml builds and uploads `mandible-%%{version}-vendor.tar.xz`
# alongside the other release assets, so Source1 resolves for anyone.
#
# `Version:` below tracks the current workspace version so the spec can be
# built by hand from a clean checkout; the release workflow rewrites it
# from the tag it is building, which is the value that reaches COPR.

# The release profile sets `strip = "symbols"` (workspace Cargo.toml), so
# there are no symbols left for rpm's debuginfo extraction to find and it
# fails the build rather than producing an empty package. Nothing here is
# a debuggable artifact, so there is no debuginfo subpackage.
%global debug_package %{nil}

%global forgeurl https://github.com/AS-FOSS/mandible

Name:           mandible
Version:        0.4.5
Release:        1%{?dist}
Summary:        Universal, interactive TUI reference for CLI tools

License:        MIT OR Apache-2.0
URL:            %{forgeurl}
Source0:        %{forgeurl}/archive/refs/tags/v%{version}/%{name}-%{version}.tar.gz
Source1:        %{forgeurl}/releases/download/v%{version}/%{name}-%{version}-vendor.tar.xz

BuildRequires:  gcc
BuildRequires:  cargo >= 1.88
BuildRequires:  rust >= 1.88

%description
mandible is a universal, interactive TUI reference for CLI tools.
`mandible git` opens an explorable tree of every command, subcommand, and
flag git has, with descriptions — a reference browser, not a command
builder.

It reads what a tool says about itself, so it works on tools nobody wrote
a page for. Parsing is keyed by framework rather than by tool name, and
what it could not determine is shown as such rather than guessed at.

%prep
%autosetup -n %{name}-%{version}
# Source1 unpacks to ./vendor
tar -xf %{SOURCE1}

# Appended, not written over: the checked-in .cargo/config.toml carries the
# `cargo xtask` alias that this project's own docs and CI depend on, and
# replacing the file would silently delete it.
mkdir -p .cargo
cat >> .cargo/config.toml <<'EOF'

[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = "vendor"
EOF

%build
# --offline is the assertion, not just a flag: if anything were missing
# from the vendor tarball the build fails here saying so, rather than
# reaching for a network that a mock root does not have and failing later
# with something less specific.
cargo build --release --locked --offline --bin %{name}

%install
install -Dpm0755 target/release/%{name} %{buildroot}%{_bindir}/%{name}
install -Dpm0644 packaging/%{name}.1 %{buildroot}%{_mandir}/man1/%{name}.1

# The shell completions come from the binary just built, through the same
# `scripts/gen_completions.sh` the .deb and .rpm builds use, so all four
# packaging channels emit identical files from one generator. It writes each
# one under the name the shell looks for — bash-completion loads
# `completions/<command>`, so that file is `mandible`, not `mandible.bash`.
scripts/gen_completions.sh
install -Dpm0644 target/release/completions/mandible      %{buildroot}%{_datadir}/bash-completion/completions/%{name}
install -Dpm0644 target/release/completions/_mandible     %{buildroot}%{_datadir}/zsh/site-functions/_%{name}
install -Dpm0644 target/release/completions/mandible.fish %{buildroot}%{_datadir}/fish/vendor_completions.d/%{name}.fish

%check
# The one failure worth catching at build time is a binary that cannot
# start. `--doctor` exercises the real extraction pipeline and needs no
# tty, which a mock build root does not have.
target/release/%{name} --version
target/release/%{name} --doctor tar

%files
%license LICENSE-MIT LICENSE-APACHE
%doc README.md NOTICE
%{_bindir}/%{name}
%{_mandir}/man1/%{name}.1*
%{_datadir}/bash-completion/completions/%{name}
%{_datadir}/zsh/site-functions/_%{name}
%{_datadir}/fish/vendor_completions.d/%{name}.fish

%changelog
# Deliberately empty. This project's history lives in CHANGELOG.md, and
# the release body is generated from it by scripts/changelog_section.sh; a
# second hand-maintained log here would be a second thing to forget.
