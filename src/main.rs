// Prevent console window in addition to Slint window in Windows release builds
// when, e.g., starting the app via file manager. Ignored on other platforms.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

slint::slint!{
    export component HelloWorld {
        Text {
            text: "hello world";
            color: orange;
        }
    }
}

fn main() {
    HelloWorld::new().unwrap().run().unwrap();
}