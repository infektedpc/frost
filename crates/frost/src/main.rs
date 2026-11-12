//! Frost browser — libcosmic application shell.
//!
//! Round 1 is a *chrome skeleton*: the toolbar (back / forward / reload /
//! stop, address bar, hamburger menu) renders and every control is wired
//! to a [`Message`], but the messages only update the status line for now.
//! The engine is NOT wired in yet — that is Round 2, when the web content
//! area becomes a live Servo frame uploaded into a texture each spin.
//!
//! Internal pages (frost://settings, frost://print, frost://open,
//! frost://exit) are menu entries now and real pages in Round 3; they are
//! served from the embedder loopback server, following the servo-de
//! `/daemon/*` proxy pattern (see ARCHITECTURE.md).
//!
//! Build the window with:
//!   cargo run -p frost --features ui-libcosmic
//! Without the feature this binary prints a short status note and exits 0,
//! so the workspace always compiles even where libcosmic cannot build.

#[cfg(feature = "ui-libcosmic")]
mod app {
    use cosmic::app::{Core, Settings, Task};
    use cosmic::iced::Size;
    use cosmic::widget::{self, button, container, text, text_input};
    use cosmic::{executor, iced, Element};

    /// Every control in the chrome maps to one of these. Round 2 turns the
    /// navigation ones into real `frost-engine` calls; the menu ones open
    /// the internal pages (frost://settings etc.) in the active tab.
    #[derive(Clone, Debug)]
    pub enum Message {
        Back,
        Forward,
        Reload,
        Stop,
        AddressChanged(String),
        AddressSubmitted,
        ToggleMenu,
        OpenSettings,
        OpenPrint,
        OpenFile,
        Exit,
    }

    pub struct Frost {
        core: Core,
        /// What the address bar shows (the URL being typed, or the current
        /// page URL once the engine is wired in).
        address: String,
        /// One-line status shown under the toolbar (load state in Round 2).
        status: String,
        /// Whether the hamburger menu is open.
        menu_open: bool,
    }

    impl cosmic::Application for Frost {
        type Executor = executor::Default;
        type Flags = ();
        type Message = Message;

        const APP_ID: &'static str = "dev.frost.Frost";

        fn core(&self) -> &Core {
            &self.core
        }

        fn core_mut(&mut self) -> &mut Core {
            &mut self.core
        }

        fn init(core: Core, _flags: ()) -> (Self, Task<Message>) {
            (
                Self {
                    core,
                    address: "https://example.org".to_string(),
                    status: "Ready — engine wiring lands in Round 2.".to_string(),
                    menu_open: false,
                },
                Task::none(),
            )
        }

        fn update(&mut self, message: Message) -> Task<Message> {
            match message {
                Message::Back => self.status = "Back (Round 2: engine.go_back)".into(),
                Message::Forward => self.status = "Forward (Round 2: engine.go_forward)".into(),
                Message::Reload => self.status = "Reload (Round 2: engine.reload)".into(),
                Message::Stop => {
                    self.status =
                        "Stop is disabled until fork patch F-001 (no WebView::stop yet)".into()
                }
                Message::AddressChanged(value) => self.address = value,
                Message::AddressSubmitted => {
                    self.status = format!("Navigate to {} (Round 2: engine.load)", self.address)
                }
                Message::ToggleMenu => self.menu_open = !self.menu_open,
                Message::OpenSettings => {
                    self.menu_open = false;
                    self.status = "Settings (Round 3: frost://settings page)".into();
                }
                Message::OpenPrint => {
                    self.menu_open = false;
                    self.status = "Print (Round 3: frost://print page)".into();
                }
                Message::OpenFile => {
                    self.menu_open = false;
                    self.status = "Open file (Round 3: frost://open page)".into();
                }
                Message::Exit => std::process::exit(0),
            }
            Task::none()
        }

        fn view(&self) -> Element<'_, Message> {
            // Toolbar: back / forward / reload / stop, address bar, menu.
            let toolbar = widget::row::with_children(vec![
                button::standard("‹").on_press(Message::Back).into(),
                button::standard("›").on_press(Message::Forward).into(),
                button::standard("⟳").on_press(Message::Reload).into(),
                // Stop stays visible but inert until F-001 (see update()).
                button::standard("✕").on_press(Message::Stop).into(),
                text_input("Address", &self.address)
                    .on_input(Message::AddressChanged)
                    .on_submit(|_| Message::AddressSubmitted)
                    .width(iced::Length::Fill)
                    .into(),
                button::standard("☰").on_press(Message::ToggleMenu).into(),
            ])
            .spacing(8)
            .padding(8);

            // Hamburger menu: a plain column of entries for now. Round 3
            // replaces it with a proper popover menu; the entries already
            // map to the internal pages (frost://settings etc.).
            let menu: Element<Message> = if self.menu_open {
                widget::column::with_children(vec![
                    button::text("Settings").on_press(Message::OpenSettings).into(),
                    button::text("Print page…").on_press(Message::OpenPrint).into(),
                    button::text("Open file…").on_press(Message::OpenFile).into(),
                    button::text("Exit").on_press(Message::Exit).into(),
                ])
                .spacing(4)
                .padding(8)
                .into()
            } else {
                widget::column::with_children(vec![]).into()
            };

            // Web content area: a placeholder box until Round 2 uploads
            // live Servo frames here as a texture.
            let content = container(
                text("Web content appears here in Round 2 (live Servo frame).")
                    .size(16),
            )
            .width(iced::Length::Fill)
            .height(iced::Length::Fill)
            .center_x(iced::Length::Fill)
            .center_y(iced::Length::Fill);

            let status = container(text(&self.status).size(13))
                .width(iced::Length::Fill)
                .padding(6);

            widget::column::with_children(vec![
                toolbar.into(),
                menu,
                content.into(),
                status.into(),
            ])
            .into()
        }
    }

    pub fn run() -> Result<(), Box<dyn std::error::Error>> {
        let settings = Settings::default().size(Size::new(1280., 800.));
        cosmic::app::run::<Frost>(settings, ())?;
        Ok(())
    }
}

#[cfg(feature = "ui-libcosmic")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    app::run()
}

#[cfg(not(feature = "ui-libcosmic"))]
fn main() {
    eprintln!("frost: libcosmic UI is not enabled in this build.");
    eprintln!("frost: run with `cargo run -p frost --features ui-libcosmic` for the window.");
    eprintln!("frost: the engine is headless — prove it with:");
    eprintln!("frost:   cargo run -p frost-engine --example smoke");
}
