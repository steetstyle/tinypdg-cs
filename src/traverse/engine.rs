use std::io::{self, BufRead, Write};

use anyhow::Result;

use crate::resolve::types::TypeGraph;
use crate::analysis::callgraph::CallGraph;
use crate::traverse::types::*;
use crate::traverse::display;

pub fn run(state: &mut TraversalState, tg: &TypeGraph, cg: &CallGraph) -> Result<()> {
    let stdin = io::stdin();

    loop {
        display::print_header(state);
        display::print_code(state, tg, cg);
        println!("  {} Down (calls):", "↓");
        display::print_nav(&state.down, "↓");
        println!("  {} Up (called by):", "↑");
        display::print_nav(&state.up, "↑");

        display::print_prompt();

        let mut input = String::new();
        stdin.lock().read_line(&mut input)?;
        let input = input.trim();

        match parse_action(input) {
            Action::Down(idx) => {
                state.navigate_down(idx, tg, cg);
            }
            Action::DownDispatch(idx, sub) => {
                state.navigate_down_dispatch(idx, sub, tg, cg);
            }
            Action::Up(idx) => {
                state.navigate_up(idx, tg, cg);
            }
            Action::UpDispatch(idx, sub) => {
                state.navigate_up_dispatch(idx, sub, tg, cg);
            }
            Action::Complete(judgment) => {
                print!("  Evidence (one line): ");
                stdout().flush()?;
                let mut ev = String::new();
                stdin.lock().read_line(&mut ev)?;
                state.complete(judgment, ev.trim().to_string());
                println!();
                if !state.next_in_queue(tg, cg) {
                    println!("  Queue empty — traversal complete.");
                    println!();
                    display::print_history(state);
                    return Ok(());
                }
            }
            Action::Discard => {
                state.discard();
                if !state.next_in_queue(tg, cg) {
                    println!("  Queue empty — traversal complete.");
                    println!();
                    display::print_history(state);
                    return Ok(());
                }
            }
            Action::History => {
                display::print_history(state);
                print!("  (press Enter to continue)");
                stdout().flush()?;
                let mut _pause = String::new();
                stdin.lock().read_line(&mut _pause)?;
            }
            Action::Quit => {
                println!();
                display::print_history(state);
                return Ok(());
            }
            Action::Unknown => {
                println!("  Unknown action. Use ↓<n> ↑<n> c(p|s|u) d h q");
            }
        }
    }
}

#[derive(Debug)]
enum Action {
    Down(usize),
    DownDispatch(usize, usize),
    Up(usize),
    UpDispatch(usize, usize),
    Complete(Judgment),
    Discard,
    History,
    Quit,
    Unknown,
}

fn parse_action(input: &str) -> Action {
    let input = input.trim().to_lowercase();

    if input == "q" || input == "quit" { return Action::Quit; }
    if input == "h" || input == "history" { return Action::History; }
    if input == "d" || input == "disc" || input == "discard" { return Action::Discard; }

    if input.starts_with('c') {
        let rest = input[1..].trim();
        let judgment = match rest {
            "p" | "primary" => Judgment::Primary,
            "s" | "symptom" => Judgment::Symptom,
            "u" | "unrelated" => Judgment::Unrelated,
            _ => return Action::Unknown,
        };
        return Action::Complete(judgment);
    }

    if input.starts_with('u') {
        let rest = input[1..].trim();
        let num: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if let Ok(n) = num.parse::<usize>() {
            let after_num = &rest[num.len()..];
            // Check for letter suffix: u1a, u2b, etc.
            if let Some(ch) = after_num.chars().next() {
                if ch.is_ascii_alphabetic() {
                    let sub = (ch as u8 - b'a') as usize;
                    return Action::UpDispatch(n, sub);
                }
            }
            return Action::Up(n);
        }
    }

    // Check for digit+letter suffix: 5a, 3b, etc.
    let num: String = input.chars().take_while(|c| c.is_ascii_digit()).collect();
    if let Ok(n) = num.parse::<usize>() {
        let after_num = &input[num.len()..].trim();
        if let Some(ch) = after_num.chars().next() {
            if ch.is_ascii_alphabetic() {
                let sub = (ch as u8 - b'a') as usize;
                return Action::DownDispatch(n, sub);
            }
        }
        return Action::Down(n);
    }

    Action::Unknown
}

fn stdout() -> io::Stdout {
    io::stdout()
}
