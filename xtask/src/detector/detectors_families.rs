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

// Six round-4 detectors, added after the seven above. Atlas ids S-106
// through S-111, in the order the shapes were reviewed:
// underscore-in-long-option, usage-alternative-or-prefix,
// usage-program-word-mismatch, multi-operand-usage-tail,
// or-joined-alias-with-values, glued-optional-group-spelling.

pub(crate) struct UnderscoreInLongOption;

impl Detector for UnderscoreInLongOption {
    fn name(&self) -> &'static str {
        "underscore-in-long-option"
    }
    fn family(&self) -> Option<&'static str> {
        Some("underscore-in-long-option")
    }
    fn describes(&self) -> &'static str {
        "the raw text documents a `--name_with_underscore` long option token, and no entity \
         anywhere carries that full long name"
    }
    fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String> {
        super::underscore_in_long_option::detect(evidence.raw, evidence.root)
            .findings
            .iter()
            .map(|f| {
                format!(
                    "{:?} never became a long spelling, from the line {:?}",
                    f.token, f.line
                )
            })
            .collect()
    }
    fn self_checks(&self) -> Vec<SelfCheck> {
        super::underscore_in_long_option::self_checks()
    }
}

pub(crate) struct UsageAlternativeOrPrefix;

impl Detector for UsageAlternativeOrPrefix {
    fn name(&self) -> &'static str {
        "usage-alternative-or-prefix"
    }
    fn family(&self) -> Option<&'static str> {
        Some("usage-alternative-or-prefix")
    }
    fn describes(&self) -> &'static str {
        "a usage form in the tree begins with the continuation marker `or:`/`or ` still inside \
         it, instead of being stripped before the form reached the tree"
    }
    fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String> {
        super::usage_alternative_or_prefix::detect(evidence.raw, evidence.root)
            .findings
            .iter()
            .map(|f| format!("usage form still carries its `or` marker: {:?}", f.line))
            .collect()
    }
    fn self_checks(&self) -> Vec<SelfCheck> {
        super::usage_alternative_or_prefix::self_checks()
    }
}

pub(crate) struct UsageProgramWordMismatch;

impl Detector for UsageProgramWordMismatch {
    fn name(&self) -> &'static str {
        "usage-program-word-mismatch"
    }
    fn family(&self) -> Option<&'static str> {
        Some("usage-program-word-mismatch")
    }
    fn describes(&self) -> &'static str {
        "a usage form's leading bare-word run names the tool under a different spelling (a path \
         or a dotted stem) with no word equal to the node's own name"
    }
    fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String> {
        super::usage_program_word_mismatch::detect(evidence.raw, evidence.root)
            .findings
            .iter()
            .map(|f| {
                format!(
                    "{:?} never matched the node's own name, from the form {:?}",
                    f.token, f.line
                )
            })
            .collect()
    }
    fn self_checks(&self) -> Vec<SelfCheck> {
        super::usage_program_word_mismatch::self_checks()
    }
}

pub(crate) struct MultiOperandUsageTail;

impl Detector for MultiOperandUsageTail {
    fn name(&self) -> &'static str {
        "multi-operand-usage-tail"
    }
    fn family(&self) -> Option<&'static str> {
        Some("multi-operand-usage-tail")
    }
    fn describes(&self) -> &'static str {
        "the usage line's own trailing run of two or more operands, bracketed or bare, names \
         more operands than the tree's positional list carries"
    }
    fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String> {
        super::multi_operand_usage_tail::detect(evidence.raw, evidence.root)
            .findings
            .iter()
            .map(|f| {
                format!(
                    "{:?} never became a positional, from the usage line {:?}",
                    f.operand, f.usage_line
                )
            })
            .collect()
    }
    fn self_checks(&self) -> Vec<SelfCheck> {
        super::multi_operand_usage_tail::self_checks()
    }
}

pub(crate) struct OrJoinedAliasWithValues;

impl Detector for OrJoinedAliasWithValues {
    fn name(&self) -> &'static str {
        "or-joined-alias-with-values"
    }
    fn family(&self) -> Option<&'static str> {
        Some("or-joined-alias-with-values")
    }
    fn describes(&self) -> &'static str {
        "an `or`-joined alias row where both spellings document a value: the long spelling is \
         missing from the tree, or the short spelling's value name is the fabricated literal `or`"
    }
    fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String> {
        super::or_joined_alias_with_values::detect(evidence.raw, evidence.root)
            .findings
            .iter()
            .map(|f| {
                format!(
                    "{:?} beside {:?} never became one entity, from the line {:?}",
                    f.short, f.long, f.line
                )
            })
            .collect()
    }
    fn self_checks(&self) -> Vec<SelfCheck> {
        super::or_joined_alias_with_values::self_checks()
    }
}

pub(crate) struct GluedOptionalGroupSpelling;

impl Detector for GluedOptionalGroupSpelling {
    fn name(&self) -> &'static str {
        "glued-optional-group-spelling"
    }
    fn family(&self) -> Option<&'static str> {
        Some("glued-optional-group-spelling")
    }
    fn describes(&self) -> &'static str {
        "a flag documented as two or more glued optional groups (`-X[A][B]`) reaches the tree \
         with a value name that is not the source's own bracket spelling"
    }
    fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String> {
        super::glued_optional_group_spelling::detect(evidence.raw, evidence.root)
            .findings
            .iter()
            .map(|f| {
                format!(
                    "-{} carries {:?} instead of the source spelling {:?}, from the line {:?}",
                    f.flag, f.value_name, f.source_spelling, f.line
                )
            })
            .collect()
    }
    fn self_checks(&self) -> Vec<SelfCheck> {
        super::glued_optional_group_spelling::self_checks()
    }
}
