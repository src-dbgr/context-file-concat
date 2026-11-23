//! Counts the tokens of a text.
//!
//! The count is the one of the `cl100k_base` encoding, special tokens included. The
//! encoder is built once per process (building it takes longer than encoding a small
//! document) and the text is only borrowed.
//!
//! A long text is counted in pieces, on several threads. This changes nothing about
//! the result (see [`pieces`]), but it is what makes the count cancellable: the
//! cancellation flag is looked at between two pieces, so a count nobody waits for any
//! more ends after about a megabyte instead of running through the whole text.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tiktoken_rs::cl100k_base_singleton;

/// About how much text is counted between two looks at the cancellation flag.
const PIECE_SIZE: usize = 1024 * 1024;

/// The most threads a count uses.
///
/// The encoder allocates per word, so the threads share the allocator and the count
/// does not scale with the cores: measured for 50 MB of source code (on 12
/// performance cores), 1 thread takes 2.5 s, 2 take 1.6 s, 4 take 1.0 s, 6 take 0.8 s
/// and 8 take 1.0 s again. Four threads get most of what there is to get.
const MAX_WORKERS: usize = 4;

/// How many threads a count uses on a machine with `cores` cores: half of them, so
/// that the rest of the application keeps running, and at most [`MAX_WORKERS`].
fn workers_for(cores: usize) -> usize {
    (cores / 2).clamp(1, MAX_WORKERS)
}

/// Counts the tokens of `text`.
///
/// Returns `None` if `cancel_flag` was raised before the count was complete. Blocks
/// for the time the count takes (roughly 50 ms per megabyte on one thread), so it
/// must not be called on the async runtime.
pub fn count_tokens(text: &str, cancel_flag: &AtomicBool) -> Option<usize> {
    let encoder = cl100k_base_singleton();
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    count_pieces(
        &pieces(text, PIECE_SIZE),
        workers_for(cores),
        cancel_flag,
        |piece| encoder.encode_with_special_tokens(piece).len(),
    )
}

/// Adds up `count` over `pieces` on up to `workers` threads, or returns `None` once
/// `cancel_flag` is raised. Each thread looks at the flag before it takes a piece.
fn count_pieces(
    pieces: &[&str],
    workers: usize,
    cancel_flag: &AtomicBool,
    count: impl Fn(&str) -> usize + Sync,
) -> Option<usize> {
    let next = AtomicUsize::new(0);
    let total = AtomicUsize::new(0);
    let work = || {
        while !cancel_flag.load(Ordering::SeqCst) {
            let Some(piece) = pieces.get(next.fetch_add(1, Ordering::SeqCst)) else {
                break;
            };
            total.fetch_add(count(piece), Ordering::SeqCst);
        }
    };

    let helpers = workers.min(pieces.len()).saturating_sub(1);
    if helpers == 0 {
        work();
    } else {
        std::thread::scope(|scope| {
            for _ in 0..helpers {
                scope.spawn(work);
            }
            work();
        });
    }

    (!cancel_flag.load(Ordering::SeqCst)).then(|| total.into_inner())
}

/// Splits `text` into consecutive pieces of about `piece_size` bytes whose token
/// counts add up to the count of the whole text.
///
/// The encoding first cuts a text into words (runs of letters, of digits, of
/// punctuation with the line ends that follow, of whitespace) and then encodes each
/// word by itself, so a text may be cut wherever such a word ends in any case. That
/// is true right after a line end that is followed by something other than
/// whitespace: no word reaches across that place, and the words before it do not
/// depend on what follows it. A piece therefore ends at the first such place after
/// `piece_size` bytes; a text without one (a single long line) stays in one piece.
///
/// A line is therefore never divided, however long it is: a line of 20 MiB is one
/// piece, counted by one thread in about a second, and not cancellable meanwhile.
/// Cutting inside a line would need the places where the encoding ends a word there,
/// which only the encoding itself can tell.
fn pieces(text: &str, piece_size: usize) -> Vec<&str> {
    let mut pieces = Vec::with_capacity(text.len() / piece_size.max(1) + 1);
    let mut rest = text;
    while rest.len() > piece_size {
        let Some(end) = split_point(rest, piece_size) else {
            break;
        };
        let (piece, tail) = rest.split_at(end);
        pieces.push(piece);
        rest = tail;
    }
    if !rest.is_empty() {
        pieces.push(rest);
    }
    pieces
}

/// The first place at or after byte `from` where `text` may be cut: behind a line
/// feed, in front of a character that is not whitespace. `None` if there is none
/// before the end of the text.
fn split_point(text: &str, from: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    // The byte before the cut is the line feed, so the search starts one earlier.
    let mut search = from.saturating_sub(1);
    loop {
        let line_feed = search + bytes.get(search..)?.iter().position(|&b| b == b'\n')?;
        // A line feed is a character of its own, so this is a character boundary.
        let after = line_feed + 1;
        let next = text[after..].chars().next()?;
        if !next.is_whitespace() {
            return Some(after);
        }
        search = after;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn count_whole(text: &str) -> usize {
        cl100k_base_singleton()
            .encode_with_special_tokens(text)
            .len()
    }

    fn count_in_pieces(text: &str, piece_size: usize) -> usize {
        pieces(text, piece_size).into_iter().map(count_whole).sum()
    }

    #[test]
    fn counts_the_tokens_of_a_text() {
        let never = AtomicBool::new(false);
        assert_eq!(count_tokens("", &never), Some(0));
        assert_eq!(count_tokens("hello world", &never), Some(2));
        // Special tokens count as the one token they are.
        assert_eq!(count_tokens("<|endoftext|>", &never), Some(1));
    }

    #[test]
    fn pieces_are_the_text_cut_behind_line_feeds() {
        let text = "one\ntwo\n\n  three\nfour";
        assert_eq!(pieces(text, 1), vec!["one\n", "two\n\n  three\n", "four"]);
        assert_eq!(pieces(text, 5), vec!["one\ntwo\n\n  three\n", "four"]);
        assert_eq!(pieces(text, 1000), vec![text]);
        assert_eq!(pieces("", 4), Vec::<&str>::new());
        // No place to cut: one piece, however long.
        assert_eq!(pieces("a long line without an end", 4).len(), 1);
        assert_eq!(pieces("ends with line feeds\n\n\n", 4).len(), 1);
        // A piece never ends inside a character.
        assert_eq!(pieces("äöü\näöü\nx", 2), vec!["äöü\n", "äöü\n", "x"]);
    }

    #[test]
    fn a_long_text_is_counted_like_the_text_as_a_whole() {
        let source = include_str!("token_counter.rs");
        let text = source.repeat(200);
        let pieces_of_text = pieces(&text, 64 * 1024);
        assert!(pieces_of_text.len() > 10);
        assert_eq!(pieces_of_text.concat(), text);

        let whole = count_whole(&text);
        assert_eq!(count_in_pieces(&text, 64 * 1024), whole);
        assert_eq!(count_tokens(&text, &AtomicBool::new(false)), Some(whole));
    }

    #[test]
    fn whitespace_and_special_tokens_at_the_cuts_do_not_change_the_count() {
        let texts = [
            "a \n\n  \nb\n \n\t\nc \r\n\r\nd\n",
            "x;\n\n\ny = 1;\n}\n\n\n{\n",
            "end.\n<|endoftext|>\n<|endoftext|>next\n  <|endoftext|>\n",
            "tabs\t\n\tindented\n\u{a0}\nnbsp\n\u{2028}separator\n",
            "it's\n'll\n's\n12345\n67\n ünïcödé\n日本語\n🙂\n",
            "\n\n\n\nleading\n",
            "trailing   \n   \n",
        ];
        for text in texts {
            for piece_size in 1..text.len() {
                assert_eq!(
                    count_in_pieces(text, piece_size),
                    count_whole(text),
                    "{text:?} in pieces of {piece_size}"
                );
            }
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(400))]

        /// Text made of everything the encoding tells apart, with many line ends:
        /// wherever it is cut into pieces, the counts add up to the count of the whole.
        #[test]
        fn pieces_add_up_to_the_count_of_the_whole(
            parts in prop::collection::vec(
                prop_oneof![
                    Just("\n"), Just("\n"), Just("\r\n"), Just(" "), Just("  "), Just("\t"),
                    Just("word"), Just("Über"), Just("x"), Just("42"), Just("1234567"),
                    Just(";"), Just("});"), Just("'s"), Just("'"), Just("\""), Just("//"),
                    Just("<|endoftext|>"), Just("<|"), Just("|>"), Just("日本"), Just("🙂"),
                    Just("\u{a0}"), Just("\u{2028}"), Just("\u{b}"), Just("_"), Just("."),
                ],
                0..60,
            ),
            piece_size in 1usize..24,
        ) {
            let text = parts.concat();
            let cut = pieces(&text, piece_size);
            prop_assert_eq!(cut.concat(), text.clone());
            prop_assert_eq!(count_in_pieces(&text, piece_size), count_whole(&text));
        }
    }

    #[test]
    fn a_cancelled_count_has_no_result_and_counts_nothing() {
        let cancelled = AtomicBool::new(true);
        let counted = AtomicUsize::new(0);
        let pieces = ["a\n"; 100];

        let result = count_pieces(&pieces, 4, &cancelled, |_| {
            counted.fetch_add(1, Ordering::SeqCst);
            1
        });

        assert_eq!(result, None);
        assert_eq!(counted.into_inner(), 0);
        assert_eq!(count_tokens("some text", &cancelled), None);
    }

    #[test]
    fn cancelling_stops_a_count_between_two_pieces() {
        for workers in [1, 4] {
            let cancel_flag = AtomicBool::new(false);
            let counted = AtomicUsize::new(0);
            let pieces = ["a\n"; 1000];

            // The third piece is the one during which the count is cancelled.
            let result = count_pieces(&pieces, workers, &cancel_flag, |_| {
                if counted.fetch_add(1, Ordering::SeqCst) == 2 {
                    cancel_flag.store(true, Ordering::SeqCst);
                }
                1
            });

            assert_eq!(result, None);
            // Each thread finishes at most the piece it is working on.
            let counted = counted.into_inner();
            assert!(
                (3..3 + workers).contains(&counted),
                "{counted} pieces counted by {workers} threads"
            );
        }
    }

    #[test]
    fn a_count_uses_half_of_the_cores_within_its_bounds() {
        assert_eq!(workers_for(0), 1);
        assert_eq!(workers_for(1), 1);
        assert_eq!(workers_for(2), 1);
        assert_eq!(workers_for(4), 2);
        assert_eq!(workers_for(8), 4);
        assert_eq!(workers_for(64), MAX_WORKERS);
    }

    #[test]
    fn pieces_are_counted_once_each_by_any_number_of_threads() {
        let pieces: Vec<&str> = (0..257).map(|_| "piece\n").collect();
        for workers in [0, 1, 2, 8, 1000] {
            let result = count_pieces(&pieces, workers, &AtomicBool::new(false), |piece| {
                piece.len()
            });
            assert_eq!(result, Some(257 * 6), "{workers} threads");
        }
        assert_eq!(
            count_pieces(&[], 8, &AtomicBool::new(false), |_| 1),
            Some(0)
        );
    }
}
