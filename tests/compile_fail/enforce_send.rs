use thin_trait_object::*;
use std::rc::Rc;

define_v_table!(
    trait SomeVTable {}
);
impl<T> SomeVTable for Rc<T> {}

fn main() {
    ThinBox::<dyn SomeVTable + Send, _>::new(Rc::new(2), ());
}
