mod application;
mod render_state;

use winit::event_loop::{ControlFlow, EventLoop};
use application::{App};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Starting chris-wgpu application");
    // Create the event loop
    let event_loop = EventLoop::new()?;

    // Set the control flow to Poll or Wait depending on application needs
    event_loop.set_control_flow(ControlFlow::Wait);

    // Instantiate your application state
    let mut app = App::default();

    // Run the application by passing a mutable reference to your handler
    event_loop.run_app(&mut app)?;

    println!("Stopped chris-wgpu application");
    Ok(())
}
