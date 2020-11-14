use thin_trait_object::prelude::*;

define_v_table!(
    trait ToStringVTable {
        fn dyn_to_string(&self) -> String;
    }
);

struct Wrapper<T: ?Sized>(T);

impl<T> ToStringVTable for Wrapper<T>
where
    T: ToString + ?Sized,
{
    fn dyn_to_string(&self) -> String {
        self.0.to_string()
    }
}

fn main() {
    {
        let text = "test".to_owned();
        // Works:
        ThinBox::<dyn ToStringVTable, _>::new(Wrapper(&text), ());
        // Fails:
        ThinBox::<dyn ToStringVTable + 'static, _>::new(Wrapper(&text), ());
    };

    let erased = {
        let text = "test".to_owned();
        // Fails:
        ThinBox::<dyn ToStringVTable, _>::new(Wrapper(&text), ())
    };
    assert_eq!(erased.dyn_to_string(), "test");

    let erased = {
        let text = "test".to_owned();
        // Fails:
        ThinBoxWithoutCommon::<dyn ToStringVTable, _>::new(Wrapper(&text))
    };
    assert_eq!(erased.dyn_to_string(), "test");
}
