const TRACING_DT: &str = "DebugTerminal";

use crate::graphics::display::FrameBufferData;
use crate::graphics::text::{ColoredText, FontAtlas};
use crate::graphics::{color, fast_draw};

use super::geometry::Rect;
use tokio::sync::broadcast::error::TryRecvError;
use tracing::subscriber::SetGlobalDefaultError;
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::Registry;
use tracing_subscriber::layer::{Context, Layer};
use tokio::sync::broadcast::{Sender, Receiver, channel};
use tracing_subscriber::prelude::*;

/// A simple Debug Terminal which can be placed on the screen to provide for easy debugging.
pub struct DebugTerminal<'a> {
    /// Position of the DebugTerminal on the screen.
    rect: Rect,
    /// The message reciever; the sender is sent to the 
    /// `UILayer` struct which impliments `tracing` events.
    message_rx: Receiver<UILogLine>,
    /// Reference to the font atlas to use for drawing.
    atlas: &'a FontAtlas,
    /// Terminal font ascender height in pixels. Usually around 
    /// 70–80% of the FontAtlas `px`. 
    text_ascenders_px: usize,
    /// Terminal font decender height in pixels. Usually around 
    /// 20–30% of the FontAtlas `px`. 
    text_descender_px: usize,
    /// Internal rendering state.
    state: DebugTerminalState,
}

struct DebugTerminalState { 
    current_line: usize,
    max_lines: usize,
    is_dirty: bool,
}

impl<'a> DebugTerminal<'a> {
    /// Creates a `DebugTerminal` and hooks it up to the `tracing` registry
    /// along with the `fmt` layer. It will now accumulate up to 32 messages
    /// delievered via `tracing.`
    pub fn init(rect: Rect, font_atlas: &'a FontAtlas, text_ascenders_px: usize, text_descender_px: usize) -> Result<Self, SetGlobalDefaultError>  {
        let (terminal_tx, terminal_rx) = channel::<UILogLine>(32);

        let ui_layer = UILayer::new(terminal_tx);
        let fmt_layer = tracing_subscriber::fmt::layer();

        // NOTE: `fmt_layer` can be removed in the future if I don't need it 
        // logging to stderr.
        let subscriber = Registry::default()
            .with(fmt_layer)
            .with(ui_layer);

        // MUST be run only once per process.
        match tracing::subscriber::set_global_default(subscriber) {
            Ok(_) => (),
            Err(e) => return Err(e),
        }

        tracing::info!(target: TRACING_DT, "Initialized Successfully at {}.", rect);

        Ok(Self {
            rect: rect,
            message_rx: terminal_rx,
            text_ascenders_px: text_ascenders_px,
            text_descender_px: text_descender_px,
            atlas: font_atlas,
            state: DebugTerminalState { 
                current_line: 0,
                max_lines: rect.height as usize / (text_ascenders_px + text_descender_px),
                is_dirty: true,
            }
        })
    }

    pub fn render(
        &mut self,
        fb: &mut [u8],
        fb_data: &FrameBufferData,
    ) {
        let messages = self.collect_messages();

        // Clear terminal and redraw from the top if we have filled
        // the terminal or it is flagged as dirty.
        //
        // This does have the unfortuante side effect of clearing 
        // the entire terminal every time the rect is made dirty.
        // Do I care? Eh. It's not really that bad of an issue,
        // since this is just a debug developer terminal. And it
        // shouldn't be set to dirty very often. Or ever.
        if self.state.is_dirty || (self.state.current_line + messages.len() > self.state.max_lines) {
            fast_draw::draw_rect_xrgb8888(fb, fb_data, self.rect, color::BLACK);
            self.state.current_line = 0;
            self.state.is_dirty = false;
        }

        let row_height = self.text_ascenders_px + self.text_descender_px;

        // Print all messages.
        for message in messages {
            let text = Self::format_message(&message);
            
            text.draw_blend_xrgb8888(self.atlas, fb, fb_data, 
                self.rect.left as isize, 
                (self.rect.top as isize) + (self.state.current_line as isize) * (row_height as isize) + (self.text_ascenders_px as isize),
            );

            self.state.current_line += 1;
        }
    }

    #[inline(always)]
    fn format_message(message: &UILogLine) -> ColoredText {
        let mut text: ColoredText = ColoredText::new();
        let bracket_color = match message.level {
            Level::INFO => color::GRAY_70,
            Level::DEBUG => color::GREEN,
            Level::TRACE => color::CYAN,
            Level::ERROR => { text.push("! ", color::RED); color::RED}, 
            Level::WARN => { text.push("! ", color::ORANGE); color::ORANGE },
        };
        text.push("[", bracket_color);
        text.push(message.target, color::string_to_xrgb8888(message.target));
        text.push("] ", bracket_color);
        text.push(&message.message, color::WHITE);
        text
    }

    /// Collect all messages in the broadcast buffer, noting
    /// if we missed some because the buffer filled up
    /// since last time we tried to render. 
    fn collect_messages(&mut self) -> Vec<UILogLine> {
        const MAX_MESSAGES_PER_FRAME: usize = 500;

        let mut lines = Vec::new();
        let mut missed = 0usize;

        while lines.len() < MAX_MESSAGES_PER_FRAME {
            match self.message_rx.try_recv() {
                Ok(line) => lines.push(line),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Lagged(n)) => {
                    missed += n as usize;
                    continue;
                }
                Err(TryRecvError::Closed) => break,
            }
        }

        if missed > 0 {
            tracing::info!(target: TRACING_DT, "Missed {} messages.", missed);
        }

        lines
    }
}

#[derive(Clone)]
pub struct UILayer {
    tx: Sender<UILogLine>,
}

impl UILayer {
    pub fn new(tx: Sender<UILogLine>) -> Self { Self { tx } }
}

impl<S> Layer<S> for UILayer where S: Subscriber,
{
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let meta = event.metadata();
        let mut visitor = UiFieldVisitor::default();
        event.record(&mut visitor);

        let line = UILogLine {
            level: *meta.level(),
            target: meta.target(),
            message: visitor.message.unwrap_or("{no message}".to_string()),
            fields: visitor.fields,
        };

        let _ = self.tx.send(line);
    }
}

#[derive(Default)]
struct UiFieldVisitor {
    message: Option<String>,
    fields: Vec<(String, String)>,
}

impl tracing::field::Visit for UiFieldVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        let s = format!("{value:?}");
        if field.name() == "message" {
            // "message" is where `info!("...")` ends up in practice
            self.message = Some(trim_quotes(s));
        } else {
            self.fields.push((field.name().to_string(), s));
        }
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            self.message = Some(value.to_string());
        } else {
            self.fields.push((field.name().to_string(), value.to_string()));
        }
    }
}

#[derive(Clone, Debug)]
pub struct UILogLine {
    pub level: Level,
    pub target: &'static str,
    pub message: String,
    pub fields: Vec<(String, String)>,
}

fn trim_quotes(mut s: String) -> String {
    if s.starts_with('\"') && s.ends_with('\"') && s.len() >= 2 {
        s.remove(0);
        s.pop();
    }
    s
}