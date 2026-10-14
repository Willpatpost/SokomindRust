//! Extract Grand Hall finish phase from post-drain state
//! 
//! Post-drain state (after 242-move room route):
//! - Boxes 11,12 at 155,159 (bottom of corridor)
//! - Boxes C,D at waiting positions (row 6-7, columns 3,11)
//! - Goals 197,198,206,207 at bottom goals
//! 
//! Grand Hall grid (15x15):
//! Row 0:  OOOOOOOOOOOOOOO
//! Row 1:  OaSS   S   SSbO
//! Row 2:  OSCS  OOO  SDSO
//! Row 3:  OX X  OOO  X XO
//! Row 4:  O     OOO     O
//! Row 5:  OOOO   X   OOOO
//! Row 6:  O      O      O
//! Row 7:  O G hOOOOOH g O
//! Row 8:  O      O      O
//! Row 9:  OOO         OOO
//! Row 10: OOO   X X   OOO
//! Row 11: OOOOOOOROOOOOOO
//! Row 12: O B X X X X A O
//! Row 13: O Sc       dS O
//! Row 14: OOOOOOOOOOOOOOO

use sokomind_core::{Board, Cell};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Grand Hall post-drain finish phase extraction");
    
    // From analysis:
    // After drain: player at 172, boxes 11,12 at 155,159, C,D at ~64,86
    // Actually after drain, C,D should be in upper staging (rows 6-7)
    // and 11,12 are at 155,159 (column 10, row 12 and 13)
    
    // Let me recalculate based on Grand Hall layout
    // Row 12: O B X X X X A O  (positions 151-163)
    // Row 13: O Sc       dS O  (positions 165-177)
    // 
    // Cell indices (row * 15 + col):
    // Row 12: 151=O, 152=B, 153=X, 154=X, 155=X, 156=X, 157=A, 158=O
    // Row 13: 165=O, 166=S, 167=c, 168= , 169= , 170= , 171= , 172=d, 173=S, 174=O
    
    // Wait, I need to re-examine the drain result
    // The drain solved the bottom room completely
    // After drain: C,D should be waiting in upper region, 11,12 moved to 155,159
    
    // Let me check the actual positions after the room route
    // Room route pushes 11->155, 12->159
    // Then C,D need to come down
    
    // The finish sub-board should capture:
    // - Player coming from room (cell 172 corridor entrance)
    // - Boxes 11,12 at 155,159 
    // - Boxes C,D waiting in upper staging area
    // - Goals at 197,198,206,207
    
    // From Grand Hall: goals are at
    // a(1), b(27), c(78), d(100), S(76,77,101,102,177,178)
    // The finish goals are the 4 corner S goals in the bottom room
    
    // Let's build the sub-board from rows 6-14, cols 0-14
    // But we need to extract just the relevant section
    
    // After drain, the setup is:
    // - Player at 172 (row 11, col 10) - corridor entrance
    // - Box 11 at 155 (row 10, col 10) - actually this is before push
    // - Box 12 at 159 (row 10, col 14) - wait, let me recalculate
    
    // Grand Hall is 15 columns wide
    // Row 12: O B X X X X A O
    // 0123456789...
    // Position 155 = row 10, col 5 (10*15 + 5 = 155)
    // Position 159 = row 10, col 9 (10*15 + 9 = 159)
    
    // Actually the drain pushes 11 and 12 DOWN to the bottom goals
    // Let me trace through more carefully
    
    println!("\nPost-drain configuration:");
    println!("  After room route (242 moves):");
    println!("    - Bottom room solved (6 residents to goals)");
    println!("    - Boxes 11,12 pushed to 155,159");
    println!("    - C,D waiting in upper staging (rows 6-7)");
    println!("    - Player at corridor entrance");
    
    // Build the puzzle representing the finish phase
    // The sub-board spans the corridor and bottom room
    // Rows 6-14 (9 rows), columns 0-14 (full width)
    
    // But we need a tighter bounding box
    // The action is in columns 5-10 (the corridor)
    
    println!("\nFinish phase puzzle (sub-board):");
    let rows = vec![
        "OOOOOOOOOOOOOOO",   // row 0 - top (unchanged)
        "OaSS   S   SSbO",   // row 1
        "OSCS  OOO  SDSO",   // row 2
        "OX X  OOO  X XO",   // row 3
        "O     OOO     O",   // row 4
        "OOOO   X   OOOO",   // row 5
        "O      O      O",   // row 6 - C,D staging area
        "O      O      O",   // row 7
        "O      O      O",   // row 8
        "OOO         OOO",   // row 9
        "OOO   X X   OOO",   // row 10 - 11,12 approach
        "OOOOOOOROOOOOOO",   // row 11 - corridor, player R at 172
        "O  X X X X   O",    // row 12 - boxes 11,12 at 155,159
        "O  c     d   O",    // row 13 - C,D waiting
        "OOOOOOOOOOOOOOO",   // row 14
    ];
    
    // Actually I need to be more precise about the post-drain state
    // Let me think about what the board looks like AFTER the drain
    
    // The drain solved the bottom room completely, moving 6 boxes
    // to their goals in the bottom room (cells 176,177,185,186,197,198,206,207)
    
    // Wait, I'm confusing myself. Let me re-read the Grand Hall puzzle:
    
    println!("\nGrand Hall goal positions:");
    println!("  Goal 'a' at row 1, col 1 = cell 16");
    println!("  Goal 'b' at row 1, col 13 = cell 28");
    println!("  Goal 'c' at row 2, col 2 = cell 32");
    println!("  Goal 'd' at row 2, col 12 = cell 42");
    println!("  Goal 'S' (multiple) at bottom room");
    
    // Actually the puzzle uses different labels!
    // Let me check the actual puzzle again
    
    Ok(())
}
