use crossterm::event::{KeyCode, KeyEvent};
use glebin_protocol::{ClientMessage, MAX_CHAT_LEN};

use crate::app::{App, InputMode};

#[derive(Debug, PartialEq, Eq)]
pub enum InputAction {
    Continue,
    Quit,
    Send(ClientMessage),
}

pub fn handle_key(key: KeyEvent, app: &mut App) -> InputAction {
    if app.input_mode == InputMode::Chat {
        return handle_chat_key(key, app);
    }

    match key.code {
        KeyCode::Char('q') => InputAction::Quit,
        KeyCode::Enter | KeyCode::Char('c') => {
            app.input_mode = InputMode::Chat;
            app.status = "Chat mode".to_string();
            InputAction::Continue
        }
        KeyCode::Char('/') => {
            app.input_mode = InputMode::Chat;
            app.chat_input = "/".to_string();
            app.status = "Chat command mode".to_string();
            InputAction::Continue
        }
        KeyCode::Up | KeyCode::Char('w') => movement(0, -1, app),
        KeyCode::Down | KeyCode::Char('s') => movement(0, 1, app),
        KeyCode::Left | KeyCode::Char('a') => movement(-1, 0, app),
        KeyCode::Right | KeyCode::Char('d') => movement(1, 0, app),
        _ => InputAction::Continue,
    }
}

fn handle_chat_key(key: KeyEvent, app: &mut App) -> InputAction {
    match key.code {
        KeyCode::Esc => {
            app.input_mode = InputMode::Navigate;
            app.chat_input.clear();
            app.status = "Chat cancelled".to_string();
            InputAction::Continue
        }
        KeyCode::Enter => {
            let text = sanitize_client_text(&app.chat_input);
            app.chat_input.clear();
            app.input_mode = InputMode::Navigate;
            if text.is_empty() {
                InputAction::Continue
            } else {
                app.status = format!("You: {text}");
                InputAction::Send(ClientMessage::SendChat { text })
            }
        }
        KeyCode::Backspace => {
            app.chat_input.pop();
            InputAction::Continue
        }
        KeyCode::Char(character) => {
            if app.chat_input.chars().count() < MAX_CHAT_LEN {
                app.chat_input.push(character);
            }
            InputAction::Continue
        }
        _ => InputAction::Continue,
    }
}

fn movement(dx: i16, dy: i16, app: &mut App) -> InputAction {
    app.status = format!("Moved by ({dx}, {dy})");
    InputAction::Send(ClientMessage::Move { dx, dy })
}

pub fn sanitize_client_text(input: &str) -> String {
    input
        .trim()
        .chars()
        .filter(|ch| !ch.is_control())
        .take(MAX_CHAT_LEN)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    #[test]
    fn slash_opens_chat_with_the_command_prefix() {
        let mut app = App::new("test".to_string(), "test".to_string());
        handle_key(
            KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE),
            &mut app,
        );
        assert_eq!(app.input_mode, InputMode::Chat);
        assert_eq!(app.chat_input, "/");
    }
}
