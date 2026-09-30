use sokomind_core::{Board, MAX_BOXES, NONE, ParseError, WALL};

const FIRST: &str = "OOOOO\nO R O\nO A O\nO a O\nOOOOO";
const TWO: &str = "OOOOOO\nO R  O\nO XO O\nOO A O\nOSa  O\nOOOOOO";

#[test]
fn fingerprints_match_the_reference() {
    // Values produced by SokomindSolver's puzzleRevisionFingerprint.
    assert_eq!(
        Board::parse(FIRST).unwrap().fingerprint(),
        "puzzle-v1:5ae6cd46"
    );
    assert_eq!(
        Board::parse(TWO).unwrap().fingerprint(),
        "puzzle-v1:0d758a96"
    );
    let moved = Board::parse("OOOOO\nO R O\nO a O\nO A O\nOOOOO").unwrap();
    assert_ne!(
        moved.fingerprint(),
        Board::parse(FIRST).unwrap().fingerprint()
    );
}

#[test]
fn rejects_carriage_returns_and_empty_rows() {
    assert_eq!(
        rejection("OOOOO\r\nO R O\r\nOOOOO"),
        ParseError::CarriageReturn
    );
    assert_eq!(rejection("OOOOO\n\nO R O\nOOOOO"), ParseError::EmptyRow);
    assert_eq!(rejection("OOOOO\nO R O\nOOOOO\n\n"), ParseError::EmptyRow);
    // One trailing newline is a paste artifact, not a row.
    let board = Board::parse("OOOOO\nO R O\nO A O\nO a O\nOOOOO\n").unwrap();
    assert_eq!(board.height(), 5);
    assert_eq!(board.fingerprint(), "puzzle-v1:5ae6cd46");
}

#[test]
fn ragged_rows_are_padded_with_walls() {
    let board = Board::parse("OOOOOO\nOR XS\nOO").unwrap();
    assert_eq!((board.width(), board.height()), (6, 3));
    assert_eq!([board.tiles()[11], board.tiles()[17]], [WALL; 2]);
    // The padding blocks movement like any other wall.
    assert_eq!(board.neighbors()[10][3], NONE);
}

/// Why the text is rejected, panicking if it parses.
fn rejection(text: &str) -> ParseError {
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
    assert_eq!(Board::parse(&rows.join("\n")).unwrap().tiles().len(), 4096);
    // Ragged rows pad to the widest one: 65 x 64 cells.
    rows[63].push('O');
    assert_eq!(rejection(&rows.join("\n")), ParseError::TooManyCells);
}

#[test]
fn text_limit_is_8192_bytes() {
    let mut text = "R\nX\nS\n".to_string();
    text.push_str(&"O\n".repeat(4093));
    assert_eq!(text.len(), 8192);
    assert_eq!(Board::parse(&text).unwrap().height(), 4096);
    text.push('O');
    assert_eq!(rejection(&text), ParseError::TooLarge);
}

#[test]
fn box_count_is_1_to_32() {
    assert_eq!(
        Board::parse(&room(MAX_BOXES)).unwrap().labels().len(),
        MAX_BOXES
    );
    assert_eq!(rejection(&room(MAX_BOXES + 1)), ParseError::BoxCount);
    assert_eq!(rejection("OOO\nORO\nOOO"), ParseError::BoxCount);
    // Goals alone are not boxes.
    assert_eq!(rejection("OOOO\nORSO\nOaOO\nOOOO"), ParseError::BoxCount);
}

#[test]
fn rejects_unknown_symbols() {
    // A multi-byte character is reported at its first byte.
    for symbol in ['#', '.', '@', '$', '*', '+', '0', '\t', '\u{e9}'] {
        let text = format!("OOOOO\nOR{symbol}XO\nO  SO\nOOOOO");
        assert_eq!(
            rejection(&text),
            ParseError::UnsupportedSymbol { row: 2, column: 3 },
            "{symbol:?}"
        );
    }
}

/// No box carries O, R or S, and plain S already marks goals for X boxes,
/// so o, r, s and x are not goals. Each is reported where it stands rather
/// than as an unmatched goal after the whole board is read.
#[test]
fn reserved_goal_letters_report_their_position() {
    for (text, row, column) in [
        ("OOOOOO\nORoXSO\nO    O\nOOOOOO", 2, 3),
        ("OOOOOO\nOR XSO\nOr   O\nOOOOOO", 3, 2),
        ("OOOOOO\nOR XSO\nO  s O\nOOOOOO", 3, 4),
        ("OOOOOO\nOR XSO\nO   xO\nOOOOOO", 3, 5),
    ] {
        assert_eq!(
            rejection(text),
            ParseError::UnsupportedSymbol { row, column },
            "{text:?}"
        );
        assert_eq!(
            rejection(text).to_string(),
            format!("Unsupported symbol at row {row}, column {column}")
        );
    }
}

#[test]
fn requires_exactly_one_robot() {
    assert_eq!(rejection("OOOOO\nO XSO\nOOOOO"), ParseError::RobotCount);
    assert_eq!(rejection("OOOOOO\nORXSRO\nOOOOOO"), ParseError::RobotCount);
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
        assert_eq!(rejection(text), ParseError::UnmatchedGoals, "{text:?}");
    }
}

/// Users read these texts through the server and the web app, so they are
/// pinned here: changing one changes what users see.
#[test]
fn parse_errors_keep_their_messages() {
    let cases = [
        (ParseError::TooLarge, "Board text is too large"),
        (
            ParseError::CarriageReturn,
            "Board rows cannot contain carriage returns",
        ),
        (ParseError::EmptyRow, "Board rows cannot be empty"),
        (ParseError::TooManyCells, "Board must contain 1..4096 cells"),
        (
            ParseError::UnsupportedSymbol { row: 2, column: 3 },
            "Unsupported symbol at row 2, column 3",
        ),
        (ParseError::RobotCount, "Exactly one robot R is required"),
        (ParseError::BoxCount, "Use 1..32 boxes"),
        (
            ParseError::UnmatchedGoals,
            "Each box label must have the same number of matching goals",
        ),
    ];
    for (error, message) in cases {
        assert_eq!(error.to_string(), message);
    }
}
