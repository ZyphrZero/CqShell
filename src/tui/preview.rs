//! Command_Preview (design component 4, Requirement 7): the CLI command equivalent to running
//! one Action with its parameters (`cqs run …`) or the Checklist (`cqs batch …`), quoted for a
//! POSIX shell and wrapped between words. Nothing here draws: the view shows the lines of
//! [`wrap`] and draws the whole command in text3 while [`incomplete`] holds.
use crate::actions::{Field, Parameters, Spec};
use std::borrow::Cow;
use unicode_width::UnicodeWidthStr;

/// Continuation lines start with this (Requirement 7.8).
const INDENT: &str = "    ";
/// A line after which the command continues ends with this.
const CONTINUED: &str = " \\";

/// One word of a Command_Preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Word {
    /// An argument exactly as the CLI receives it; [`Word::shown`] quotes it.
    Arg(String),
    /// `<key>=<name>` with the literal `<name>`, for a required value that is still empty
    /// (Requirement 7.7). It is shown as is and never quoted: the command cannot run until
    /// the value is filled in, so it is not a shell word to round-trip.
    Placeholder(String),
}

impl Word {
    /// The word as the preview shows it: an argument through [`quote`], a placeholder as is.
    pub fn shown(&self) -> Cow<'_, str> {
        match self {
            Self::Arg(word) => quote(word),
            Self::Placeholder(word) => Cow::Borrowed(word),
        }
    }
}

fn arg(word: impl Into<String>) -> Word {
    Word::Arg(word.into())
}

/// `sudo` when any Action of the command needs root (Requirement 7.3), then `cqs <command>`.
fn command(root: bool, subcommand: &str) -> Vec<Word> {
    let mut words = Vec::new();
    if root {
        words.push(arg("sudo"));
    }
    words.extend([arg("cqs"), arg(subcommand)]);
    words
}

/// The value the Action runs with: a field `params` leaves out has its default, as
/// [`Spec::parameters`] gives it.
fn value<'a>(field: &'a Field, params: &'a Parameters) -> &'a str {
    params.get(field.name).map_or(field.default, String::as_str)
}

/// `--set <key>=<value>`, or `--set <key>=<name>` with the literal `<name>` while `field` is
/// required and `value` is empty (Requirement 7.7).
fn push_set(words: &mut Vec<Word>, key: &str, field: &Field, value: &str) {
    words.push(arg("--set"));
    words.push(if value.is_empty() && field.required {
        Word::Placeholder(format!("{key}=<{}>", field.name))
    } else {
        arg(format!("{key}={value}"))
    });
}

/// `cqs run <id>`, then a `--set <name>=<value>` for each non-empty value in the order of
/// `spec.fields` (not the key order of `params`), with `sudo` first when the Action needs
/// root (Requirements 7.1, 7.3).
pub(super) fn single_words(spec: &Spec, params: &Parameters) -> Vec<Word> {
    let mut words = command(spec.root, "run");
    words.push(arg(spec.id));
    for field in &spec.fields {
        let value = value(field, params);
        if !value.is_empty() || field.required {
            push_set(&mut words, field.name, field, value);
        }
    }
    words
}

/// `cqs batch` and the IDs in the order of `jobs`, which the Checklist builds in OPTIMIZATION
/// order, then a `--set <id>.<name>=<value>` for each value that differs from
/// `spec.defaults()[name]`, that is the field's default (Requirement 7.2). The CLI fills in
/// the defaults the same way, so it rebuilds these jobs (Requirement 7.6). The comparison is
/// with the defaults, not a form's initial values: terminal.apply's form starts at the
/// current options, and a value equal to those but not to the default would otherwise be
/// left out, and the CLI would run the default instead.
pub(super) fn batch_words(jobs: &[(Spec, Parameters)]) -> Vec<Word> {
    let mut words = command(jobs.iter().any(|(spec, _)| spec.root), "batch");
    words.extend(jobs.iter().map(|(spec, _)| arg(spec.id)));
    for (spec, params) in jobs {
        for field in &spec.fields {
            let value = value(field, params);
            if value != field.default || (value.is_empty() && field.required) {
                push_set(
                    &mut words,
                    &format!("{}.{}", spec.id, field.name),
                    field,
                    value,
                );
            }
        }
    }
    words
}

/// Whether a required value is still missing: the command cannot run as shown, so the view
/// draws all of it in text3 (Requirement 7.7).
pub(super) fn incomplete(words: &[Word]) -> bool {
    words
        .iter()
        .any(|word| matches!(word, Word::Placeholder(_)))
}

/// `word` as one POSIX shell word (Requirement 7.4): as is when it is non-empty and every
/// character is in `[A-Za-z0-9_./:=@%+,-]`, otherwise in single quotes with each `'` written
/// as `'\''`.
pub(super) fn quote(word: &str) -> Cow<'_, str> {
    let plain = |c: char| c.is_ascii_alphanumeric() || "_./:=@%+,-".contains(c);
    if !word.is_empty() && word.chars().all(plain) {
        Cow::Borrowed(word)
    } else {
        Cow::Owned(format!("'{}'", word.replace('\'', r"'\''")))
    }
}

/// The shown words as lines of at most `width` columns (Requirement 7.8). Lines break only
/// between words; a line after which the command continues ends with ` \`, and continuation
/// lines start with four spaces. A word too wide for a line of its own still takes one, wider
/// than `width`, so that every word stays whole.
pub(super) fn wrap<S: AsRef<str>>(words: &[S], width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    let mut used = 0;
    for (i, word) in words.iter().enumerate() {
        let word = word.as_ref();
        if i > 0 {
            // A line that ends before the last word needs room for ` \` as well.
            let continued = if i + 1 < words.len() {
                CONTINUED.width()
            } else {
                0
            };
            if used + 1 + word.width() + continued <= width {
                line.push(' ');
                line.push_str(word);
                used += 1 + word.width();
                continue;
            }
            line.push_str(CONTINUED);
            lines.push(std::mem::replace(&mut line, INDENT.to_owned()));
            used = INDENT.width();
        }
        line.push_str(word);
        used += word.width();
    }
    if !words.is_empty() {
        lines.push(line);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actions::{self, find};
    use proptest::prelude::*;

    fn shown(words: &[Word]) -> Vec<String> {
        words.iter().map(|word| word.shown().into_owned()).collect()
    }

    fn strings(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_owned()).collect()
    }

    /// A job as the Checklist runs it: `values` completed and validated by `spec.parameters`.
    fn job(id: &str, values: &[(&str, &str)]) -> (Spec, Parameters) {
        let spec = find(id).unwrap();
        let supplied = values
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect();
        let params = spec.parameters(&supplied).unwrap();
        (spec, params)
    }

    /// A minimal POSIX shell word splitter for the round trip (Requirement 7.5). It handles
    /// only what the preview writes: blanks between words, single quotes, the `\'` of `'\''`,
    /// and a backslash-newline, which POSIX removes outside quotes (the space [`wrap`] puts
    /// before it is what separates the words). Unquoted, it accepts only the characters a
    /// shell leaves literal anywhere in an argument; anything else is an error, as is a
    /// newline without a backslash, which would end the command.
    fn split(text: &str) -> Result<Vec<String>, String> {
        let mut words = Vec::new();
        let mut word: Option<String> = None;
        let mut chars = text.chars();
        while let Some(c) = chars.next() {
            match c {
                ' ' | '\t' => words.extend(word.take()),
                '\'' => {
                    let word = word.get_or_insert_with(String::new);
                    loop {
                        match chars.next() {
                            Some('\'') => break,
                            Some(c) => word.push(c),
                            None => return Err(format!("unterminated quote in {text:?}")),
                        }
                    }
                }
                '\\' => match chars.next() {
                    Some('\n') => {}
                    Some('\'') => word.get_or_insert_with(String::new).push('\''),
                    other => return Err(format!("backslash before {other:?} in {text:?}")),
                },
                c if c.is_ascii_alphanumeric() || "_./:=@%+,-".contains(c) => {
                    word.get_or_insert_with(String::new).push(c);
                }
                c => return Err(format!("unquoted {c:?} in {text:?}")),
            }
        }
        words.extend(word);
        Ok(words)
    }

    /// Printable values that mix plain characters, characters a shell treats specially and
    /// any other printable character.
    fn value(len: std::ops::Range<usize>) -> impl Strategy<Value = String> {
        let char = prop_oneof![
            proptest::sample::select("aZ09_./:=@%+,-".chars().collect::<Vec<_>>()),
            proptest::sample::select(" '\"\\$`!*?[]{}()<>|&;#~^".chars().collect::<Vec<_>>()),
            "\\PC".prop_map(|c| c.chars().next().expect("the pattern matches one character")),
        ];
        proptest::collection::vec(char, len).prop_map(String::from_iter)
    }

    /// A spec with fields and a value for every field; required values are non-empty.
    fn spec_and_params() -> impl Strategy<Value = (Spec, Parameters)> {
        let specs = actions::specs()
            .into_iter()
            .filter(|spec| !spec.fields.is_empty())
            .collect::<Vec<_>>();
        proptest::sample::select(specs)
            .prop_flat_map(|spec| {
                let values = spec
                    .fields
                    .iter()
                    .map(|field| value(if field.required { 1..12 } else { 0..12 }))
                    .collect::<Vec<_>>();
                (Just(spec), values)
            })
            .prop_map(|(spec, values)| {
                let names = spec.fields.iter().map(|field| field.name.to_owned());
                let params = names.zip(values).collect();
                (spec, params)
            })
    }

    fn optimization_jobs() -> impl Strategy<Value = Vec<(Spec, Parameters)>> {
        let jobs = actions::OPTIMIZATION
            .iter()
            .map(|id| {
                let spec = find(id).unwrap();
                let fields = spec
                    .fields
                    .iter()
                    .map(|field| {
                        let generated = if !field.choices.is_empty() && !field.allow_custom {
                            proptest::sample::select(field.choices.to_vec())
                                .prop_map(str::to_owned)
                                .boxed()
                        } else {
                            match field.name {
                                "bandwidth" => (1u32..=100_000_000)
                                    .prop_map(|value| {
                                        format!("{}.{:03}", value / 1000, value % 1000)
                                    })
                                    .boxed(),
                                "rtt" => (1u32..=10_000_000)
                                    .prop_map(|value| {
                                        format!("{}.{:03}", value / 1000, value % 1000)
                                    })
                                    .boxed(),
                                "port" => {
                                    (1u16..=u16::MAX).prop_map(|port| port.to_string()).boxed()
                                }
                                "timezone" => proptest::sample::select(vec![
                                    "UTC",
                                    "Asia/Shanghai",
                                    "Europe/London",
                                    "America/New_York",
                                    "Pacific/Auckland",
                                ])
                                .prop_map(str::to_owned)
                                .boxed(),
                                "public_key" => "[a-zA-Z0-9 '$中]{1,24}"
                                    .prop_map(|name| {
                                        std::env::temp_dir()
                                            .join(format!("{name}.pub"))
                                            .to_string_lossy()
                                            .into_owned()
                                    })
                                    .boxed(),
                                name => panic!("missing valid generator for {}.{name}", spec.id),
                            }
                        };
                        if field.required && field.default.is_empty() {
                            generated
                        } else {
                            prop_oneof![Just(field.default.to_owned()), generated].boxed()
                        }
                    })
                    .collect::<Vec<_>>();
                (Just(spec), fields).prop_map(|(spec, values)| {
                    let supplied = spec
                        .fields
                        .iter()
                        .map(|field| field.name.to_owned())
                        .zip(values)
                        .collect();
                    let params = spec
                        .parameters(&supplied)
                        .expect("generated valid parameters");
                    (spec, params)
                })
            })
            .collect::<Vec<_>>();
        (1u16..(1u16 << actions::OPTIMIZATION.len()), jobs).prop_map(|(mask, jobs)| {
            jobs.into_iter()
                .enumerate()
                .filter_map(|(index, job)| (mask & (1 << index) != 0).then_some(job))
                .collect()
        })
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        // Feature: tui-redesign, Property 6: Command_Preview 分词往返
        /// **Validates: Requirements 7.1, 7.3, 7.4, 7.5, 7.8**
        #[test]
        fn quoted_and_wrapped_previews_split_back_into_the_arguments(
            (spec, params) in spec_and_params(),
            width in 1usize..100,
        ) {
            let mut expected = Vec::new();
            if spec.root {
                expected.push("sudo".to_owned());
            }
            expected.extend(strings(&["cqs", "run", spec.id]));
            for field in &spec.fields {
                let value = &params[field.name];
                if !value.is_empty() {
                    expected.extend(["--set".to_owned(), format!("{}={value}", field.name)]);
                }
            }
            let words = single_words(&spec, &params);
            // Every required value is filled in, so there is no placeholder to leave unquoted.
            prop_assert!(!incomplete(&words));
            let words = shown(&words);
            prop_assert_eq!(split(&words.join(" ")), Ok(expected.clone()));
            prop_assert_eq!(split(&wrap(&words, width).join("\n")), Ok(expected));
        }

        // Feature: tui-redesign, Property 7: 批量命令与 CLI 作业一致
        #[test]
        fn batch_preview_rebuilds_the_same_jobs_through_the_real_cli(jobs in optimization_jobs()) {
            let words = batch_words(&jobs);
            prop_assert!(!incomplete(&words));
            let mut arguments = split(&shown(&words).join(" ")).unwrap();
            if arguments.first().is_some_and(|word| word == "sudo") {
                arguments.remove(0);
            }
            let i18n = crate::i18n::I18n::load(Some("en-US"), None).unwrap();
            let matches = crate::cli::command(&i18n).try_get_matches_from(arguments).unwrap();
            let matches = matches.subcommand_matches("batch").unwrap();
            let ids = matches.get_many::<String>("action").unwrap().map(String::as_str).collect::<Vec<_>>();
            let sets = matches.get_many::<String>("set").into_iter().flatten().map(String::as_str).collect::<Vec<_>>();
            let rebuilt = crate::cli::batch_jobs(&ids, &sets).unwrap();
            let identify = |jobs: Vec<(Spec, Parameters)>| jobs.into_iter().map(|(spec, params)| (spec.id, params)).collect::<Vec<_>>();
            prop_assert_eq!(identify(rebuilt), identify(jobs));
        }
    }

    #[test]
    fn split_handles_only_what_the_preview_writes() {
        assert_eq!(
            split(r"a 'b c'  'd'\''e'"),
            Ok(strings(&["a", "b c", "d'e"]))
        );
        assert_eq!(split("a \\\n    b ''"), Ok(strings(&["a", "b", ""])));
        for text in ["a\nb", "$x", "'open", "a\\b", "<name>"] {
            assert!(split(text).is_err(), "{text:?}");
        }
    }

    #[test]
    fn quote_leaves_plain_words_and_single_quotes_the_rest() {
        assert!(matches!(
            quote("tcp.tune.rtt=80"),
            Cow::Borrowed("tcp.tune.rtt=80")
        ));
        assert!(matches!(quote("Az09_./:=@%+,-"), Cow::Borrowed(_)));
        for (word, quoted) in [
            ("", "''"),
            ("my sub", "'my sub'"),
            ("it's", r"'it'\''s'"),
            ("~/key", "'~/key'"),
            ("$HOME", "'$HOME'"),
            ("a\"b", "'a\"b'"),
            ("中文", "'中文'"),
        ] {
            assert_eq!(quote(word), quoted, "{word:?}");
        }
    }

    #[test]
    fn single_words_follow_the_field_order_and_skip_empty_values() {
        // Parameters is a BTreeMap, whose keys run color, git, shell, style, time.
        let spec = find("terminal.apply").unwrap();
        assert_eq!(
            shown(&single_words(&spec, &spec.defaults())).join(" "),
            "cqs run terminal.apply --set shell=bash --set style=full --set color=cyan \
             --set time=off --set git=on"
        );
        // An empty optional value is left out.
        let spec = find("kernel.install").unwrap();
        let params = Parameters::from([
            ("profile".into(), "max".into()),
            ("version".into(), String::new()),
        ]);
        assert_eq!(
            shown(&single_words(&spec, &params)).join(" "),
            "sudo cqs run kernel.install --set profile=max"
        );
        // A field the parameters leave out has its default, as `Spec::parameters` gives it.
        let spec = find("tcp.tune").unwrap();
        let params = Parameters::from([("rtt".into(), "80".into())]);
        assert_eq!(
            shown(&single_words(&spec, &params)).join(" "),
            "sudo cqs run tcp.tune --set bandwidth=1000 --set rtt=80"
        );
    }

    #[test]
    fn batch_words_set_only_the_values_that_differ_from_the_defaults() {
        let jobs = [
            job("system.update", &[]),
            job("system.clean", &[("scope", "standard")]),
            job("tcp.tune", &[("rtt", "80")]),
            job("bbr.enable", &[("qdisc", "fq_codel")]),
        ];
        assert_eq!(
            shown(&batch_words(&jobs)),
            strings(&[
                "sudo",
                "cqs",
                "batch",
                "system.update",
                "system.clean",
                "tcp.tune",
                "bbr.enable",
                "--set",
                "tcp.tune.rtt=80",
                "--set",
                "bbr.enable.qdisc=fq_codel",
            ])
        );
        let defaults = [job("system.update", &[]), job("tcp.tune", &[])];
        assert_eq!(
            shown(&batch_words(&defaults)).join(" "),
            "sudo cqs batch system.update tcp.tune"
        );
        // terminal.apply's form starts at the current options, here green; the value still
        // differs from the default, cyan, so it is set. No job needs root: no `sudo`.
        let jobs = [job("terminal.apply", &[("color", "green")])];
        assert_eq!(
            shown(&batch_words(&jobs)).join(" "),
            "cqs batch terminal.apply --set terminal.apply.color=green"
        );
    }

    #[test]
    fn a_missing_required_value_shows_its_placeholder_as_is() {
        let spec = find("clash.sub.add").unwrap();
        let mut params = Parameters::from([
            ("name".into(), "my sub".into()),
            ("url".into(), String::new()),
            ("use".into(), "off".into()),
        ]);
        let words = single_words(&spec, &params);
        assert!(incomplete(&words));
        // `quote` would put the placeholder in single quotes; the preview shows it as is.
        assert_eq!(quote("url=<url>"), "'url=<url>'");
        assert_eq!(
            shown(&words).join(" "),
            "cqs run clash.sub.add --set 'name=my sub' --set url=<url> --set use=off"
        );
        params.insert("url".into(), "https://example.com/sub".into());
        assert!(!incomplete(&single_words(&spec, &params)));

        // ssh.key's public_key defaults to empty, so the batch shows its placeholder although
        // the value equals the default.
        let jobs = [(
            find("ssh.key").unwrap(),
            Parameters::from([("public_key".into(), String::new())]),
        )];
        let words = batch_words(&jobs);
        assert!(incomplete(&words));
        assert_eq!(
            shown(&words).join(" "),
            "sudo cqs batch ssh.key --set ssh.key.public_key=<public_key>"
        );
    }

    #[test]
    fn wrap_breaks_between_words_and_indents_continuation_lines() {
        let words = [
            "sudo",
            "cqs",
            "run",
            "tcp.tune",
            "--set",
            "bandwidth=1000",
            "--set",
            "rtt=80",
        ];
        assert_eq!(
            wrap(&words, 80),
            ["sudo cqs run tcp.tune --set bandwidth=1000 --set rtt=80"]
        );
        let lines = wrap(&words, 30);
        assert_eq!(
            lines,
            [
                "sudo cqs run tcp.tune --set \\",
                "    bandwidth=1000 --set \\",
                "    rtt=80",
            ]
        );
        assert!(lines.iter().all(|line| line.width() <= 30), "{lines:?}");
        assert_eq!(split(&lines.join("\n")), Ok(strings(&words)));
    }

    #[test]
    fn a_word_wider_than_the_line_takes_a_line_of_its_own() {
        let url = quote("url=https://example.com/a b");
        let words = ["cqs", "run", "clash.sub.add", "--set", &*url];
        let lines = wrap(&words, 12);
        // Only the lines holding a single word are wider than 12 columns.
        assert_eq!(
            lines,
            [
                "cqs run \\",
                "    clash.sub.add \\",
                "    --set \\",
                "    'url=https://example.com/a b'",
            ]
        );
        assert_eq!(
            split(&lines.join("\n")),
            Ok(strings(&[
                "cqs",
                "run",
                "clash.sub.add",
                "--set",
                "url=https://example.com/a b"
            ]))
        );
    }
}
