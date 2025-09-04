// Temporary binary for size analysis - imports major dependencies
use websink::websink::server::State;
use tokio::runtime::Runtime;
use webrtc::api::APIBuilder;

#[tokio::main]
async fn main() {
    println!("Analyzing dependencies...");
    
    // Force inclusion of major dependencies
    let _state = State::default();
    let _rt = Runtime::new().unwrap();
    let _api = APIBuilder::new().build();
    
    // Just reference some functions from heavy crates
    let _json = serde_json::json!({});
    let _uuid = uuid::Uuid::new_v4();
    
    println!("Analysis complete");
}
