//! `description-tail-enumerates-choices` (atlas S-156): a flag description
//! whose tail opens a labelled list and then runs comma-separated literal
//! values to the end, which belong in the flag's own choices.
//!
//! Fixture: `corpus/grub-mkimage/2.12`. Counted only, below the bar.

use mandible_core::CommandNode;

const LABELS: &[&str] = &[
    "available formats:",
    "possible values:",
    "one of:",
    "valid values:",
];

pub struct Finding {
    pub long: String,
    pub label: &'static str,
    pub members: Vec<String>,
}

pub struct Report {
    pub findings: Vec<Finding>,
}

impl Report {
    pub fn finding_count(&self) -> usize {
        self.findings.len()
    }
}

fn is_literal_member(m: &str) -> bool {
    let mut chars = m.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_lowercase() || first.is_ascii_digit())
        && chars.all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '.' | '+' | '-')
        })
}

/// The last labelled list this description's own tail carries, when the
/// label is the last one in the text and every member after it is
/// literal, at least three of them, running to the very end.
fn label_tail(description: &str) -> Option<(&'static str, Vec<String>)> {
    let lower = description.to_lowercase();
    let mut best: Option<(usize, &'static str)> = None;
    for label in LABELS {
        if let Some(idx) = lower.rfind(label) {
            if best.is_none_or(|(b, _)| idx > b) {
                best = Some((idx, label));
            }
        }
    }
    let (idx, label) = best?;
    let tail = description[idx + label.len()..].trim();
    if tail.is_empty() {
        return None;
    }
    let members: Vec<&str> = tail.split(',').map(str::trim).collect();
    if members.len() < 3 || !members.iter().all(|m| is_literal_member(m)) {
        return None;
    }
    Some((label, members.into_iter().map(str::to_string).collect()))
}

pub fn detect(_raw: &str, root: &CommandNode) -> Report {
    let mut findings = Vec::new();
    for e in root.flags() {
        let Some(description) = e.description.as_ref().map(|t| t.as_str()) else {
            continue;
        };
        let Some((label, members)) = label_tail(description) else {
            continue;
        };
        let already_attached = members.len() == e.choices.len()
            && members.iter().zip(&e.choices).all(|(m, c)| m == &c.name);
        if !already_attached {
            let Some(long) = e.long().map(str::to_string) else {
                continue;
            };
            findings.push(Finding {
                long,
                label,
                members,
            });
        }
    }
    Report { findings }
}

pub struct DescriptionTailEnumeratesChoices;

impl crate::detector::Detector for DescriptionTailEnumeratesChoices {
    fn name(&self) -> &'static str {
        "description-tail-enumerates-choices"
    }

    fn family(&self) -> Option<&'static str> {
        // No seed-7 labelled tool carries this shape (calibration verdict
        // NOT EVALUABLE), so there is no closed `DEFECT_FAMILIES` entry to
        // claim. See docs/shapes.md S-156.
        None
    }

    fn describes(&self) -> &'static str {
        "a flag description whose continuation opens a labelled list (`available formats:`) \
         and runs comma-separated literal values to the end never becomes that flag's choices"
    }

    fn hits(&self, evidence: &crate::detector::ToolEvidence<'_>) -> Vec<String> {
        detect(evidence.raw, evidence.root)
            .findings
            .into_iter()
            .map(|f| {
                format!(
                    "-{} {:?} tail never became choices: {:?}",
                    f.long, f.label, f.members
                )
            })
            .collect()
    }

    fn self_checks(&self) -> Vec<SelfCheck> {
        self_checks()
    }
}

// ----------------------------------------------------------------------
// Self-checks
// ----------------------------------------------------------------------

use crate::detector::{Expect, SelfCheck};
use mandible_core::{Choice, Entity, Provenance, Source, Text};

const GRUB_MKIMAGE_FORMAT_DESCRIPTION: &str = "generate an image in FORMAT available formats: \
     i386-coreboot, i386-multiboot, i386-pc, i386-xen_pvh, i386-pc-eltorito, i386-efi, \
     i386-ieee1275, i386-qemu, x86_64-efi, i386-xen, x86_64-xen, mipsel-yeeloong-flash, \
     mipsel-fuloong2f-flash, mipsel-loongson-elf, powerpc-ieee1275, sparc64-ieee1275-raw, \
     sparc64-ieee1275-cdcore, sparc64-ieee1275-aout, ia64-efi, mips-arc, mipsel-arc, \
     mipsel-qemu_mips-elf, mips-qemu_mips-flash, mipsel-qemu_mips-flash, mips-qemu_mips-elf, \
     arm-uboot, arm-coreboot-vexpress, arm-coreboot-veyron, arm-efi, arm64-efi, loongarch64-efi, \
     riscv32-efi, riscv64-efi";

fn flag_with(long: &str, description: &str, choices: &[&str]) -> Entity {
    let mut e = Entity::flag_spelled(
        None,
        Some(long.to_string()),
        false,
        false,
        Provenance::single(Source::HelpText),
    );
    e.description = Some(Text::sanitize(description));
    e.choices = choices.iter().map(|c| Choice::bare(*c)).collect();
    e
}

fn node_with(name: &str, flags: Vec<Entity>) -> CommandNode {
    let mut root = CommandNode::new(name, Provenance::single(Source::HelpText));
    root.set_entities_of(mandible_core::EntityKind::Flag, flags);
    root
}

pub(crate) fn self_checks() -> Vec<SelfCheck> {
    vec![
        SelfCheck {
            name: "grub-mkimage's own `--format`, pre-fix shape (list still in the description)",
            why: "the defect itself: the described list never became choices",
            expect: Expect::Fires(1),
            raw: String::new(),
            root: node_with(
                "grub-mkimage",
                vec![flag_with("format", GRUB_MKIMAGE_FORMAT_DESCRIPTION, &[])],
            ),
        },
        SelfCheck {
            name: "grub-mkimage's own `--format`, post-fix shape (choices attached, label gone)",
            why: "once the label and list move to choices, the identical tree must go silent",
            expect: Expect::Silent,
            raw: String::new(),
            root: node_with(
                "grub-mkimage",
                vec![flag_with(
                    "format",
                    "generate an image in FORMAT",
                    &[
                        "i386-coreboot",
                        "i386-multiboot",
                        "i386-pc",
                        "i386-xen_pvh",
                        "i386-pc-eltorito",
                        "i386-efi",
                        "i386-ieee1275",
                        "i386-qemu",
                        "x86_64-efi",
                        "i386-xen",
                        "x86_64-xen",
                        "mipsel-yeeloong-flash",
                        "mipsel-fuloong2f-flash",
                        "mipsel-loongson-elf",
                        "powerpc-ieee1275",
                        "sparc64-ieee1275-raw",
                        "sparc64-ieee1275-cdcore",
                        "sparc64-ieee1275-aout",
                        "ia64-efi",
                        "mips-arc",
                        "mipsel-arc",
                        "mipsel-qemu_mips-elf",
                        "mips-qemu_mips-flash",
                        "mipsel-qemu_mips-flash",
                        "mips-qemu_mips-elf",
                        "arm-uboot",
                        "arm-coreboot-vexpress",
                        "arm-coreboot-veyron",
                        "arm-efi",
                        "arm64-efi",
                        "loongarch64-efi",
                        "riscv32-efi",
                        "riscv64-efi",
                    ],
                )],
            ),
        },
        SelfCheck {
            name: "an ordinary description with no labelled list at all",
            why: "prose that never opens one of the four labels must never be claimed",
            expect: Expect::Silent,
            raw: String::new(),
            root: node_with(
                "prog",
                vec![flag_with("target", "the triple to build for", &[])],
            ),
        },
        SelfCheck {
            name: "a labelled list with fewer than three members",
            why: "the gate requires at least three members; two is too weak a signal to trust",
            expect: Expect::Silent,
            raw: String::new(),
            root: node_with(
                "prog",
                vec![flag_with("mode", "select a mode one of: fast, slow", &[])],
            ),
        },
        SelfCheck {
            name: "a labelled list followed by more prose",
            why: "the label must be the last sentence-opening; text after the list means this \
                  is not the tail of the description and must never be claimed",
            expect: Expect::Silent,
            raw: String::new(),
            root: node_with(
                "prog",
                vec![flag_with(
                    "mode",
                    "select a mode one of: fast, slow, medium. See the manual for details",
                    &[],
                )],
            ),
        },
    ]
}
