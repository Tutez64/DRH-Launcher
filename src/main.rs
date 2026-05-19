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