use sokomind_core::Board;

const FIRST: &str = "OOOOO\nO R O\nO A O\nO a O\nOOOOO";
const TWO: &str = "OOOOOO\nO R  O\nO XO O\nOO A O\nOSa  O\nOOOOOO";

#[test]
fn fingerprints_match_the_reference() {
    // Values produced by SokomindSolver's puzzleRevisionFingerprint.
    assert_eq!(Board::parse(FIRST).unwrap().fingerprint, "puzzle-v1:5ae6cd46");
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
