//! Analyze Grand Hall to find finish phase configuration
//! 
//! Grand Hall is 15x15:
//!  0123456789abcde
//! 0 OOOOOOOOOOOOOOO
//! 1 OaSS   S   SSbO  goals: a(16), b(28), S(17,18,23,24,25,26,27)
//! 2 OSCS  OOO  SDSO  goals: c(32), d(42), S(33,34,41,42)
//! 3 OX X  OOO  X XO
//! 4 O     OOO     O
//! 5 OOOO   X   OOOO
//! 6 O      O      O
//! 7 O G hOOOOOH g O  goals: G(48), H(54), g(49), h(53)
//! 8 O      O      O
//! 9 OOO         OOO
//! 10 OOO   X X   OOO
//! 11 OOOOOOOROOOOOOO  player R at cell 172 (row 11, col 10)
//! 12 O B X X X X A O  boxes: B(152), A(157)
//! 13 O Sc       dS O  boxes: c(167), d(173), S(166,177)
//! 14 OOOOOOOOOOOOOOO
//! 
//! Total boxes: 17 (a,b,c,d,A,B + 11 others marked as X or S in initial)
//! 
//! The "huge" puzzle uses:
//! - Lowercase a,b,c,d,g,h as GOALS (matching lowercase)
//! - Uppercase A,B as BOXES (matching uppercase)
//! - S as goal that matches... wait, S goals should have matching boxes
