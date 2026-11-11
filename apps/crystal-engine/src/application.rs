use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowId};
use crate::render_state::RenderState;

#[derive(Default)]
pub struct App {
    state: Option<RenderState>,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_none() {
            let window_attributes = Window::default_attributes().with_title("WGPU Clear Color");
            let window = event_loop.create_window(window_attributes).unwrap();

            // Block on the async setup function to populate state natively
            let state = pollster::block_on(RenderState::new(window));
            self.state = Some(state);
        }
    }

    // 2. This is where you handle all window events (clicks, typing, closing, resizing)
    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        // Safe check to ensure our graphics state is initialized before routing events
        let state = match &mut self.state {
            Some(s) => s,
            None => return,
        };

        match event {
            // Handle the close button (X) being clicked
            WindowEvent::CloseRequested => {
                println!("The close button was clicked; exiting...");
                event_loop.exit();
            }
            WindowEvent::Resized(physical_size) => {
                state.resize(physical_size);
            }
            // Request a redraw when needed
            WindowEvent::RedrawRequested => {
                // Simply fire off the render method and ignore non-fatal frame skips
                if let Err(err) = state.render() {
                    match err {
                        "Timeout" | "Occluded" | "Outdated" | "Lost" => {
                            // These are normal lifecycle skips or handled via auto-resize inline
                            // We can safely ignore them or log them as warnings
                        }
                        _ => eprintln!("Render error: {}", err),
                    }
                }
            }
            _ => (),
        }
    }
}