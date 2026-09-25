use sokomind_core::{Board, MAX_BOXES};

const FIRST: &str = "OOOOO\nO R O\nO A O\nO a O\nOOOOO";
const TWO: &str = "OOOOOO\nO R  O\nO XO O\nOO A O\nOSa  O\nOOOOOO";

#[test]
fn fingerprints_match_the_reference() {
    // Values produced by SokomindSolver's puzzleRevisionFingerprint.
    assert_eq!(
        Board::parse(FIRST).unwrap().fingerprint,
        "puzzle-v1:5ae6cd46"
    );
    assert_eq!(Board::parse(TWO).unwrap().fingerprint, "puzzle-v1:0d758a96");
    let moved = Board::parse("OOOOO\nO R O\nO a O\nO A O\nOOOOO").unwrap();
    assert_ne!(moved.fingerprint, Board::parse(FIRST).unwrap().fingerprint);
}

#[test]
fn rejects_carriage_returns_and_empty_rows() {
    assert!(Board::parse("OOOOO\r\nO R O\r\nOOOOO").is_err());
    assert!(Board::parse("OOOOO\n\nO R O\nOOOOO").is_err());
    assert!(Board::parse("OOOOO\nO R O\nOOOOO\n\n").is_err());
    // One trailing newline is a paste artifact, not a row.
    let board = Board::parse("OOOOO\nO R O\nO A O\nO a O\nOOOOO\n").unwrap();
    assert_eq!(board.height, 5);
    assert_eq!(board.fingerprint, "puzzle-v1:5ae6cd46");
}

/// The rejection message, panicking if the text parses.
fn rejection(text: &str) -> String {
    Board::parse(text)
        .err()
        .unwrap_or_else(|| panic!("accepted {text:?}"))
}

/// A walled room with `count` X boxes above as many goals.
fn room(count: usize) -> String {
    let wall = "O".repeat(count + 2);
    let floor = " ".repeat(count - 1);
    let boxes = "X".repeat(count);
    let goals = "S".repeat(count);
    format!("{wall}\nOR{floor}O\nO{boxes}O\nO{goals}O\n{wall}")
}

#[test]
fn cell_limit_is_4096() {
    let mut rows = vec!["O".repeat(64); 64];
    rows[1] = format!("ORXS{}", "O".repeat(60));
    assert_eq!(Board::parse(&rows.join("\n")).unwrap().tiles.len(), 4096);
    // Ragged rows pad to the widest one: 65 x 64 cells.
    rows[63].push('O');
    assert!(rejection(&rows.join("\n")).contains("1..4096 cells"));
}

#[test]
fn text_limit_is_8192_bytes() {
    let mut text = "R\nX\nS\n".to_string();
    text.push_str(&"O\n".repeat(4093));
    assert_eq!(text.len(), 8192);
    assert_eq!(Board::parse(&text).unwrap().height, 4096);
    text.push('O');
    assert!(rejection(&text).contains("too large"));
}

#[test]
fn box_count_is_1_to_32() {
    assert_eq!(
        Board::parse(&room(MAX_BOXES)).unwrap().labels.len(),
        MAX_BOXES
    );
    assert!(rejection(&room(MAX_BOXES + 1)).contains("1..32 boxes"));
    assert!(rejection("OOO\nORO\nOOO").contains("1..32 boxes"));
    // Goals alone are not boxes.
    assert!(rejection("OOOO\nORSO\nOaOO\nOOOO").contains("1..32 boxes"));
}

#[test]
fn rejects_unknown_symbols() {
    // Lowercase x is reserved: plain S already marks goals for X boxes.
    for symbol in ['#', '.', '@', '$', '*', '+', '0', 'x', '\t', '\u{e9}'] {
        let text = format!("OOOOO\nOR{symbol}XO\nO  SO\nOOOOO");
        assert!(
            rejection(&text).contains("Unsupported symbol at row 2, column 3"),
            "{symbol:?}"
        );
    }
}

#[test]
fn requires_exactly_one_robot() {
    assert!(rejection("OOOOO\nO XSO\nOOOOO").contains("Exactly one robot"));
    assert!(rejection("OOOOOO\nORXSRO\nOOOOOO").contains("Exactly one robot"));
}

#[test]
fn every_label_needs_as_many_goals_as_boxes() {
    for text in [
        "OOOOO\nORAbO\nOOOOO",
        "OOOOO\nORXaO\nOOOOO",
        "OOOOOO\nORXSSO\nOOOOOO",
        "OOOOOO\nORXXSO\nOOOOOO",
        "OOOOOO\nORAaaO\nOOOOOO",
    ] {
        assert!(
            rejection(text).contains("same number of matching goals"),
            "{text:?}"
        );
    }
}
