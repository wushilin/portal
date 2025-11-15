use crate::util::uuid;


pub mod util;


#[tokio::main]
async fn main() {
    let mut extendable = util::Extendable::new(42);
    extendable.attach(String::from("hello"));
    extendable.attach(String::from("hello111"));
    extendable.attach(123);
    let m = extendable.get_mut::<String>();
    if let Some(m) = m {
        *m = String::from(uuid().to_string());
    }
    extendable.replace(String::from("world"));
    extendable.remove::<i32>();
    let result = extendable.take::<String>();
    if let Some(result) = result {
        println!("taken: {}", result);
    }
    println!("{:?}", extendable.get::<String>());
    println!("{:?}", extendable.get::<i32>());
}