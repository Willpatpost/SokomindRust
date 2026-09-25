use sokomind_core::{Board, Game};
use sokomind_search::{ExactSearch, Proof, Status};

/// Frozen move-optima ported from the reference solver's fixtures
/// (`tests/fixtures/solver-v2/{known-optima,benchmark-corpus,tunnel-soundness,
/// pattern-deadlock-soundness}.ts`). Each move count is the reference's
/// independent step-oracle output; push counts are deliberately not asserted
/// because move-optimal search does not optimize them. Boards whose optima
/// the reference only established with machinery this port does not carry yet
/// (`expert-tetris`, the large catalog boards) stay out of the gate until the
/// exact kernel can reach them.
struct Fixture {
    rows: &'static str,
    moves: Option<u32>,
}

fn check(id: &str, fixture: &Fixture) {
    let board = Board::parse(fixture.rows).unwrap();
    let mut engine = ExactSearch::new(board.clone(), board.initial, 1_000_000, 256).unwrap();
    while engine.status() == Status::Running {
        engine.advance(4096);
    }
    match fixture.moves {
        Some(moves) => {
            assert_eq!(engine.status(), Status::Solved, "{id}");
            assert_eq!(engine.best_moves(), Some(moves), "{id}");
            assert_eq!(engine.proof(), Some(Proof::Optimal { moves }), "{id}");
            let route = engine.solution().unwrap().unwrap();
            let mut game = Game::new(board);
            game.replay(&route).unwrap();
            assert!(game.solved(), "{id}");
            assert_eq!(route.len(), moves as usize, "{id}");
        }
        None => {
            assert_eq!(engine.status(), Status::Exhausted, "{id}");
            assert_eq!(engine.best_moves(), None, "{id}");
            assert_eq!(engine.proof(), Some(Proof::Unsolvable), "{id}");
            assert_eq!(engine.solution().unwrap(), None, "{id}");
        }
    }
}

macro_rules! optimum {
    ($name:ident, $rows:expr, $moves:expr) => {
        #[test]
        fn $name() {
            check(
                stringify!($name),
                &Fixture {
                    rows: $rows,
                    moves: Some($moves),
                },
            );
        }
    };
}
macro_rules! unsolvable {
    ($name:ident, $rows:expr) => {
        #[test]
        fn $name() {
            check(stringify!($name), &Fixture { rows: $rows, moves: None });
        }
    };
}

// --- Reference catalog snapshots ------------------------------------------

optimum!(ultra_tiny, "OOOOO\nO R O\nO A O\nO a O\nOOOOO", 1);
optimum!(tiny, "OOOOOO\nO R  O\nO XO O\nOO A O\nOSa  O\nOOOOOO", 20);
optimum!(
    tutorial_push,
    "OOOOO\nO XSO\nO   O\nO R O\nOOOOO",
    4
);
optimum!(
    tutorial_corner,
    "OOOOOO\nO    O\nO RX O\nO  S O\nO    O\nOOOOOO",
    3
);
optimum!(
    tutorial_around,
    "OOOOOOO\nOR    O\nOOOOX O\nO   S O\nOOOOOOO",
    4
);
optimum!(
    beginner_three,
    "OOOOOOOO\nO R    O\nO XXXO O\nO SSSO O\nO      O\nOOOOOOOO",
    7
);
optimum!(
    beginner_detour,
    "OOOOOOOO\nOR     O\nOOOO X O\nOS   X O\nOS     O\nOOOOOOOO",
    24
);
optimum!(
    beginner_typed_line,
    "OOOOOOOOO\nOc b a  O\nO       O\nO A B C O\nO   R   O\nOOOOOOOOO",
    27
);
optimum!(
    garden_1,
    "OOOOOOOOO\nO   R   O\nO A B C O\nO       O\nO a b c O\nOOOOOOOOO",
    16
);
optimum!(box_5x5_a, "OOOOO\nOSX O\nO XRO\nO  SO\nOOOOO", 6);
optimum!(
    medium,
    "OOOOOOO\nOa   bO\nO AXB O\nO XRX O\nOSCXDSO\nOcS SdO\nOOOOOOO",
    34
);
optimum!(
    inter_rooms,
    "OOOOOOOOOOO\nO    O    O\nO RX   XS O\nO XO O OX O\nOSSO   OS O\nOOOOOOOOOOO",
    28
);
optimum!(
    corridor_2,
    "OOOOOOOOOOO\nO S O     O\nO   O X   O\nO     R   O\nO   O X   O\nO S O     O\nOOOOO X   O\nOOOOOO  S O\nOOOOOOOOOOO",
    41
);
optimum!(
    garden_2,
    "OOOOOOOOOOO\nO    R    O\nO OOO OOO O\nO A     B O\nO OOO OOO O\nO  b   a  O\nO OO O OO O\nO         O\nOOOOOOOOOOO",
    73
);
optimum!(
    workshop_1,
    "OOOOOOO\nO   R O\nO OXO O\nO X   O\nOSX   O\nOS    O\nOS    O\nOOOOOOO",
    23
);
optimum!(
    classic_1,
    "OOOOOOO\nO     O\nO OXO O\nO  X  O\nOO X OO\nO  R  O\nO SSS O\nOOOOOOO",
    40
);
optimum!(
    theme_kitchen,
    "OOOOOOOOO\nO R     O\nO  OOO  O\nO X O X O\nO  O    O\nO  O  X O\nO SSS   O\nOOOOOOOOO",
    34
);
optimum!(
    adv_rotary,
    "OOOOOOOOOOO\nOOa  ROOOOO\nOO  OO  bOO\nO A    B  O\nO   OO    O\nOOOOOOOOOOO",
    17
);
optimum!(
    adv_four_color,
    "OOOOOOOOO\nOb     cO\nO       O\nO  C  D O\nO   R   O\nO  A  B O\nO       O\nOd     aO\nOOOOOOOOO",
    43
);
optimum!(
    adv_gallery,
    "OOOOOOOOOO\nO R      O\nO OOOOOO O\nO O    O O\nO X SS X O\nO O    O O\nO OXOOXO O\nO        O\nO   SS   O\nOOOOOOOOOO",
    29
);
optimum!(
    box_7x7,
    "OOOOOOO\nOS   SO\nO  X  O\nO XRXOO\nO  X  O\nOS   SO\nOOOOOOO",
    21
);
optimum!(
    sym_diamond,
    "OOOOOOOOOOO\nOOOOO OOOOO\nOOOO   OOOO\nOOO  S  OOO\nOO  XRX  OO\nO    X    O\nOO   S   OO\nOOO  S  OOO\nOOOO   OOOO\nOOOOO OOOOO\nOOOOOOOOOOO",
    16
);
optimum!(
    theme_library,
    "OOOOOOOOOOO\nOaaO R ObbO\nO  O   O  O\nO  OO OO  O\nO   A B   O\nO  A   B  O\nO         O\nOOOOOOOOOOO",
    45
);
optimum!(
    expert_maze,
    "OOOOOOOOOOOO\nO R  O     O\nOOO  O OOO O\nO X  O O S O\nO OO   O   O\nO O  OOOO  O\nO   XO  X  O\nOOOO OS    O\nO  X    OO O\nO SSS X    O\nOOOOOOOOOOOO",
    65
);
optimum!(
    expert_tetris,
    "OOOOOOOOO\nO   R   O\nO  X X  O\nOOX   XOO\nOO     OO\nOO X X OO\nOOSSSSSOO\nOO  S  OO\nOOOOOOOOO",
    38
);

// --- Reference supplemental fixtures --------------------------------------

optimum!(
    v2_microban_145,
    "OOOOOO\nO    O\nO XX O\nO  X O\nO R  O\nOSSSOO\nOOOOOO",
    23
);
optimum!(
    v2_microban_146,
    "OOOOOOO\nOS R SO\nO  XX O\nOO O OO\nO     O\nOO X OO\nOO S OO\nOOOOOOO",
    23
);
optimum!(
    v2_caleb_022,
    "OOOOOOOO\nO R    O\nO  OX  O\nO    X O\nOOX  OOO\nOSS    O\nOSS X OO\nOOOOOOOO",
    45
);
optimum!(
    v2_solved_box_must_move,
    "OOOOOOO\nO     O\nOOS R O\nO X   O\nO X   O\nO S   O\nOOOOOOO",
    14
);
optimum!(
    v2_assignment_infeasible,
    "OOOOOOO\nO   R O\nO X   O\nOOO   O\nO S X O\nO S   O\nOOOOOOO",
    17
);
optimum!(
    v2_sealed_corral,
    "OOOOOOO\nO R   O\nO X X O\nOOO OOO\nO  S  O\nO  S  O\nOOOOOOO",
    19
);
optimum!(
    v2_wide_multi_entry,
    "OOOOOOOOOOOOO\nO    O      O\nOR X   S    O\nO    OOOOO OO\nO  X   S    O\nO    OOOOO OO\nO  X   S    O\nO    O      O\nOOOOOOOOOOOOO",
    25
);
optimum!(
    v2_loop_heavy,
    "OOOOOOOOOOOOO\nO   R       O\nO O O O O O O\nO     X     O\nO O O O O O O\nO           O\nO O O O O O O\nO     X     O\nO O O O O O O\nO  S     S  O\nOOOOOOOOOOOOO",
    32
);

// --- Reference soundness boards --------------------------------------------
// Tunnel-macro and pattern-deadlock regressions from the reference. This port
// does not carry those mechanisms, but the boards must still produce their
// oracle optima under any pruning it does have.

optimum!(
    es01,
    "OOOOOOOOO\nOOcOOOOOO\nOOCOOOOOO\nORA    aO\nOXOOOOOOO\nOSOOOOOOO\nO OOOOOOO\nOOOOOOOOO",
    9
);
optimum!(
    es01a,
    "OOOOOOOOOO\nOOO      O\nOOO OOOO O\nOSRX     O\nOOOOOOOOOO",
    16
);
optimum!(
    es01c,
    "OOOOOOOOOO\nOOO      O\nOOO OOOO O\nOSRX     O\nOO OOOOOOO\nOO XSOOOOO\nOO   OOOOO\nOOOOOOOOOO",
    19
);
optimum!(
    es01d,
    "OOOOOOOOOO\nOOO      O\nOOO OOOO O\nOSRX     O\nOO OOOOOOO\nO     OOOO\nO X S OOOO\nO     OOOO\nOOOOOOOOOO",
    22
);
optimum!(
    fz_unsolvable,
    "OOOOOOOO\nO      O\nO OOO aO\nO    AaO\nOOOOOOAO\nOOOOOORO\nOOOOOOOO",
    15
);
optimum!(
    fz_corridor,
    "OOOOO\nO  aO\nO   O\nOOO O\nO O O\nObB O\nOSXAO\nO R O\nOOOOO",
    10
);
optimum!(
    pd_false_optimum,
    "OOOOOOOOOOOOOOOOOOOOOOO\nOO  OOO  OOOOOOOOOOOOOO\nOO                R OOO\nOO OOOO O OOOOOOO O OOO\nOO OOOO O OOOOOOO O OOO\nOO OOOO O OOOOOOO O OOO\nOO OOOO O OOOOOOOXO OOO\nOS        XSO       XSO\nOOOOOOOOOOOOOOOOOOOOOOO",
    63
);
optimum!(
    pd_false_unsolvable,
    "OOOOOOOOOOOOOOOOOOOOOOO\nOOOOOOO  OOOOOOOOOOOOOO\nOOOOOOO           R OOO\nOOOOOOO O OOOOOOO O OOO\nOOOOOOO O OOOOOOO O OOO\nOOOOOOO O OOOOOOO O OOO\nOOOOOOO O OOOOOOOXO OOO\nOS        XSO       XSO\nOOOOOOOOOOOOOOOOOOOOOOO",
    63
);

// B can only be pushed deeper into the one-wide corridor and the robot can
// never get behind it, so A can never pass B. No box starts on a dead square,
// so only an exhausted frontier proves it.
unsolvable!(
    blocked_typed_corridor,
    "OOOOOOOOO\nOabB    O\nOOOO    O\nOOOO A  O\nOOOO  R O\nOOOOOOOOO"
);
