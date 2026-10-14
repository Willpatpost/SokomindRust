use sokomind_core::{Board, Game};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let rows = vec![
        "OOOOOOOOOOOOOOO",
        "OaSS   S   SSbO",
        "OSCS  OOO  SDSO",
        "OX X  OOO  X XO",
        "O     OOO     O",
        "OOOO   X   OOOO",
        "O      O      O",
        "O G hOOOOOH g O",
        "O      O      O",
        "OOO         OOO",
        "OOO   X X   OOO",
        "OOOOOOOROOOOOOO",
        "O B X X X X A O",
        "O Sc       dS O",
        "OOOOOOOOOOOOOOO"
    ];
    
    let board = Board::parse(&rows.join("\n"))?;
    
    let route = "DDLLURDRUUULURRRDRULLLDDDDRRULRDRRULLRRDRRULLDLLLLUUULULLDRRURRLDDDLLDLLURRRDRUUURURRDLRRUULDLLLLLDLUUUUUUURULRDDDRRRDRRDDRDLLLLLRDDDLLLLDLLURRRRRDRUDRRULLUULLURRRRDLULDDDRRDRRULLLDLLLLLRRRRUUURULLLDLUUUUUULLLURDRRUURULLLDRLDLDRRUUDRDDDDDDLDRRRURDDDRDLLLLRURDRUUURRRUUUUUUUULURRRDLLDDDDRRDLDLUUUUULURRLDDRUDRRULUDLULDDDDDDDRDLLLULLLDLUUUUUUDDLLDLDRRURDLDRRRULDLURRDRDDDLDRRRRLLLUUUURRLURDRDRUUDLUURLUUUULURDDDLLLLDLUUUULURRLDDLLDLUU";
    
    let mut game = Game::new(board);
    game.replay(route)?;
    
    println!("Route: {} moves", game.moves());
    println!("Pushes: {}", game.pushes());
    println!("Solved: {}", game.solved());
    
    if game.solved() {
        println!("Route is VALID!");
    }
    
    Ok(())
}
