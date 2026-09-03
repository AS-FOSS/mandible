//! The seven vim-family detectors, each generalizing one shape from
//! `corpus/vim.basic/audit-seed4/meta.toml`'s six maintainer-found
//! defects plus `corpus/nvim/0.9.5/` and `corpus/jinfo/17.0.20/`. Atlas
//! ids: S-095 (plus-prefixed-option), S-096 (end-of-options-marker),
//! S-097 (second-optional-value-dropped), S-098
//! (parenthetical-qualifier-as-value), S-099 (or-joined-alias), S-100
//! (usage-only-value-name, the value-name half), S-105
//! (single-space-description-column) — six of the seven already had atlas
//! entries from an earlier read of `corpus/vim.basic/audit-seed4/`; only
//! jinfo's shape was new.
use super::*;

pub(crate) struct PlusPrefixedOption;

impl Detector for PlusPrefixedOption {
    fn name(&self) -> &'static str {
        "plus-prefixed-option"
    }
    fn family(&self) -> Option<&'static str> {
        Some("plus-prefixed-option")
    }
    fn describes(&self) -> &'static str {
        "an indented option row whose leading token is `+` alone or `+<placeholder>`, opening a \
         real description column, with no entity anywhere spelled that exact token"
    }
    fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String> {
        crate::plus_prefixed_option::detect(evidence.raw, evidence.root)
            .findings
            .iter()
            .map(|f| {
                format!(
                    "{:?} never became a flag, from the line {:?}",
                    f.token, f.line
                )
            })
            .collect()
    }
    fn self_checks(&self) -> Vec<SelfCheck> {
        crate::plus_prefixed_option::self_checks()
    }
}

pub(crate) struct EndOfOptionsMarker;

impl Detector for EndOfOptionsMarker {
    fn name(&self) -> &'static str {
        "end-of-options-marker"
    }
    fn family(&self) -> Option<&'static str> {
        Some("end-of-options-marker")
    }
    fn describes(&self) -> &'static str {
        "an indented option row whose leading token is bare `--`, opening a real description \
         column, with no entity anywhere spelled `--`"
    }
    fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String> {
        crate::end_of_options_marker::detect(evidence.raw, evidence.root)
            .findings
            .iter()
            .map(|f| format!("`--` never became a flag, from the line {:?}", f.line))
            .collect()
    }
    fn self_checks(&self) -> Vec<SelfCheck> {
        crate::end_of_options_marker::self_checks()
    }
}

pub(crate) struct SingleSpaceDescriptionColumn;

impl Detector for SingleSpaceDescriptionColumn {
    fn name(&self) -> &'static str {
        "single-space-description-column"
    }
    fn family(&self) -> Option<&'static str> {
        Some("single-space-description-column")
    }
    fn describes(&self) -> &'static str {
        "a `\" | \"`-joined alias row whose last spelling is followed by exactly one space \
         before real description text, so no entity in the group carries that text as its own \
         description"
    }
    fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String> {
        crate::single_space_description_column::detect(evidence.raw, evidence.root)
            .findings
            .iter()
            .map(|f| format!("{:?} never attached to {:?}", f.description, f.spellings))
            .collect()
    }
    fn self_checks(&self) -> Vec<SelfCheck> {
        crate::single_space_description_column::self_checks()
    }
}

pub(crate) struct UsageOnlyValueName;

impl Detector for UsageOnlyValueName {
    fn name(&self) -> &'static str {
        "usage-only-value-name"
    }
    fn family(&self) -> Option<&'static str> {
        Some("usage-only-value-name")
    }
    fn describes(&self) -> &'static str {
        "a short flag in the usage block's own tokens immediately followed by a lower-case-led \
         value word, where the tree's entity for that flag carries no value name at all"
    }
    fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String> {
        crate::usage_only_value_name::detect(evidence.raw, evidence.root)
            .findings
            .iter()
            .map(|f| {
                format!(
                    "-{} never carried value name {:?}, from the usage line {:?}",
                    f.flag, f.value_name, f.usage_line
                )
            })
            .collect()
    }
    fn self_checks(&self) -> Vec<SelfCheck> {
        crate::usage_only_value_name::self_checks()
    }
}

pub(crate) struct SecondOptionalValueDropped;

impl Detector for SecondOptionalValueDropped {
    fn name(&self) -> &'static str {
        "second-optional-value-dropped"
    }
    fn family(&self) -> Option<&'static str> {
        Some("second-optional-value-dropped")
    }
    fn describes(&self) -> &'static str {
        "a leading token shaped `-<letter>[value1][value2]`, two adjacent bracketed optional \
         values, where the tree's value name for that flag never mentions the second value"
    }
    fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String> {
        crate::second_optional_value_dropped::detect(evidence.raw, evidence.root)
            .findings
            .iter()
            .map(|f| {
                format!(
                    "-{} kept {:?} but lost {:?}",
                    f.flag, f.first_value, f.second_value
                )
            })
            .collect()
    }
    fn self_checks(&self) -> Vec<SelfCheck> {
        crate::second_optional_value_dropped::self_checks()
    }
}

pub(crate) struct ParentheticalQualifierAsValue;

impl Detector for ParentheticalQualifierAsValue {
    fn name(&self) -> &'static str {
        "parenthetical-qualifier-as-value"
    }
    fn family(&self) -> Option<&'static str> {
        Some("parenthetical-qualifier-as-value")
    }
    fn describes(&self) -> &'static str {
        "a bare short flag immediately followed by a parenthetical qualifier `(...)`, where the \
         tree's value name for that flag begins with an open paren — the qualifier's leading \
         word misread as the value"
    }
    fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String> {
        crate::parenthetical_qualifier_as_value::detect(evidence.raw, evidence.root)
            .findings
            .iter()
            .map(|f| format!("-{} carries value name {:?}", f.flag, f.value_name))
            .collect()
    }
    fn self_checks(&self) -> Vec<SelfCheck> {
        crate::parenthetical_qualifier_as_value::self_checks()
    }
}

pub(crate) struct OrJoinedAlias;

impl Detector for OrJoinedAlias {
    fn name(&self) -> &'static str {
        "or-joined-alias"
    }
    fn family(&self) -> Option<&'static str> {
        Some("or-joined-alias")
    }
    fn describes(&self) -> &'static str {
        "an indented row shaped `<flag>  or  <flag>`, where the tree carries no entity spelled \
         the second flag at all"
    }
    fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String> {
        crate::or_joined_alias::detect(evidence.raw, evidence.root)
            .findings
            .iter()
            .map(|f| {
                format!(
                    "{:?} never became an alias of {:?}, from the line {:?}",
                    f.second, f.first, f.line
                )
            })
            .collect()
    }
    fn self_checks(&self) -> Vec<SelfCheck> {
        crate::or_joined_alias::self_checks()
    }
}
