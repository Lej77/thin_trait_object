//! An experiment to provide a safe API for storing trait objects using thin
//! pointers instead of fat pointers.
//!
//! Inspired from code in the [`anyhow`](https://crates.io/crates/anyhow) crate.
//! Specifically [code from version 1.0.33](https://github.com/dtolnay/anyhow/tree/c25be95f1a24f7497d2b4530ffcb4f90d3871975).
//!
//! Check out the [`prelude`] module for the most important items when using this
//! crate.
//!
//! # Examples
//!
//! ```
//! use thin_trait_object::{define_v_table, ThinBox, Thin};
//!
//! use core::mem;
//!
//! define_v_table!(
//!     trait VTableEq {
//!         fn is_equal(&self, number: u32) -> bool;
//!     }
//! );
//! impl VTableEq for u32 {
//!     fn is_equal(&self, number: u32) -> bool { *self == number }
//! }
//!
//! // While a Box<dyn Trait> has the size of 2 usize this is only 1 usize large:
//! assert_eq!(mem::size_of::<usize>(), mem::size_of::<ThinBox::<'_, dyn VTableEq, bool>>());
//! assert_eq!(mem::size_of::<usize>() * 2, mem::size_of::<Box<dyn VTableEq>>());
//!
//! // Need to specify the trait that the provided value implements (but the actual
//! // type is erased/forgotten):
//! let mut erased = ThinBox::<'_, dyn VTableEq, bool>::new(2, false);
//!
//! // ThinBox implements the `VTableEq` trait:
//! assert!(erased.is_equal(2));
//! assert!(!erased.is_equal(3));
//! // Can split a reference into the "common data" part and the type erased object
//! // part:
//! {
//!     let (erased, common) = Thin::split_common(&erased);
//!     assert!(erased.is_equal(2));
//!     assert!(!common);
//! }
//! // Can also split a mutable reference:
//! {
//!     let (erased, common) = Thin::split_common_mut(&mut erased);
//!     assert!(erased.is_equal(2));
//!     *common = true;
//! }
//! // Can move the "common data" out of the allocation and still continue using
//! // it to interact with the type erased object:
//! let (erased, common) = ThinBox::take_common(erased);
//! assert!(common);
//! assert!(erased.is_equal(2));
//! ```
//!
//! # License
//!
//! This project is released under either:
//!
//! - [MIT License](https://github.com/Lej77/cast_trait_object/blob/master/LICENSE-MIT)
//! - [Apache License (Version 2.0)](https://github.com/Lej77/cast_trait_object/blob/master/LICENSE-APACHE)
//!
//! at your choosing.

#![cfg_attr(all(not(test), not(feature = "std")), no_std)]
// Warnings and docs:
#![warn(clippy::all)]
#![deny(broken_intra_doc_links)]
#![cfg_attr(feature = "docs", feature(doc_cfg))]
#![warn(missing_debug_implementations, missing_docs, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(
        deny(warnings, rust_2018_idioms),
        allow(unused_extern_crates, unused_variables)
    )
))]
#![allow(clippy::type_complexity)]

extern crate alloc;
use alloc::boxed::Box;

use core::{
    fmt,
    marker::PhantomData,
    mem,
    mem::ManuallyDrop,
    ops::{Deref, DerefMut},
    ptr::{self, NonNull},
};

#[cfg(test)]
mod tests;

pub mod prelude {
    //! This contains the items you would normally want when using a thin pointer.

    #[doc(inline)]
    pub use super::{
        define_v_table, OwnedThin, Thin, ThinBox, ThinBoxWithoutCommon, ThinWithoutCommon,
    };
}

macro_rules! get_type_name {
    ($type:ident) => {{
        #[allow(unused_imports)]
        use $type as __AValidType;
        stringify!($type)
    }};
}

// Not public API.
//
// The parsing in this macro was inspired by code from the `pin-project-lite`
// crate: https://github.com/taiki-e/pin-project-lite
// Specifically code from:
// https://github.com/taiki-e/pin-project-lite/blob/3b7efb6584ae68f21aaf0c8742f5883a5cabc6ac/src/lib.rs#L986-L1015
#[doc(hidden)]
#[macro_export]
macro_rules! __define_v_table_internal {
    ////////////////////////////////////////////////////////////////////////////////
    // The entry point of the macro.
    ////////////////////////////////////////////////////////////////////////////////
    (@input
        $(#[$trait_attr:meta])*
        $visibility:vis $(unsafe $(;;; $is_unsafe_trait:ident)?)? trait $trait_name:ident
        $(<
            $( $lifetime:lifetime $(: $lifetime_bound:lifetime)? ),* $(,)?
            $( $generics:ident
                $(: $generics_bound:path)?
                $(: ?$generics_unsized_bound:path)?
                $(: $generics_lifetime_bound:lifetime)?
                $(= $generics_default:ty)?
            ),* $(,)?
        >)?
        $(:
            $($(+)? $super_lifetime_bound:lifetime )*
            $($(+)? $super_bound:path )*
        )?
        $(where
            $( $where_clause_ty:ty
                $(: $where_clause_bound:path)?
                $(: ?$where_clause_unsized_bound:path)?
                $(: $where_clause_lifetime_bound:lifetime)?
            ),* $(,)?
        )?
        {
             $($trait_items:tt)*
        }
    ) => {
        $crate::__define_v_table_internal!{@parse_items
            trait_def = {
                $(#[$trait_attr])*
                $visibility $(unsafe $($is_unsafe_trait)?)? trait $trait_name
                $(<
                    $( $lifetime $(: $lifetime_bound)? ,)*
                    $( $generics
                        $(: $generics_bound)?
                        $(: ?$generics_unsized_bound)?
                        $(: $generics_lifetime_bound)?
                        $(= $generics_default)?
                    ,)*
                >)?
                $(:
                    $(+ $super_lifetime_bound )*
                    $(+ $super_bound )*
                )?
                $(where
                    $( $where_clause_ty
                        $(: $where_clause_bound)?
                        $(: ?$where_clause_unsized_bound)?
                        $(: $where_clause_lifetime_bound)?
                    ,)*
                )? {}
            },
            unparsed_items = { $($trait_items)* },
            parsed_fns = {},
            parsed_associated_types = {},
        }
    };
    ////////////////////////////////////////////////////////////////////////////////
    // Parse: trait method
    ////////////////////////////////////////////////////////////////////////////////
    (@parse_items
        trait_def = $trait_def:tt,
        unparsed_items = {
            // Attributes:
            $(#[$attr:meta])*
            // Method definition:
            $(unsafe $(;;; $is_unsafe:ident)?)? fn $method_name:ident
            // Lifetime Parameters:
            $(<
                $( $lifetime:lifetime $(: $lifetime_bound:lifetime)? ),* $(,)?
            >)?
            // parameters with shorthand self (&self, &mut self, self)
            $((
                $(&$($self_life:lifetime)?)? $(mut $(;;; $self_is_mut_ref:ident)?)? self $(,$arg_name:ident: $arg_ty:ty)* $(,)?
            ))?
            // parameters with long typed self (self: &Self, self: &mut Self, self: Self)
            $((
                $(mut $(;;; $self_is_mut_binding:ident)?)? self: $(&$($self_life_adv:lifetime)?)? $(mut $(;;; $self_is_mut_ref_adv:ident)?)? Self $(,$arg_name_adv:ident: $arg_ty_adv:ty)* $(,)?
            ))?
            // Return type
            $(-> $return_type:ty)?
            // Where clause (this is a compiler warning for "object safe" methods, methods without `where Self: Sized`):
            $(where
                $( $where_clause_ty:ty
                    $(: $where_clause_bound:path)?
                    $(: ?$where_clause_unsized_bound:path)?
                    $(: $where_clause_lifetime_bound:lifetime)?
                ),* $(,)?
            )?
            // End token:
            ;
            // Next trait item:
            $($unparsed_rest:tt)*
        },
        parsed_fns = { $($parsed:tt)* },
        parsed_associated_types = $parsed_types:tt $(,)?
    ) => {
        $crate::__define_v_table_internal! {@parse_items
            trait_def = $trait_def,
            unparsed_items = { $($unparsed_rest)* },
            parsed_fns = { $($parsed)* {
                is_unsafe = {  $(unsafe $(;;; $is_unsafe)?)?  },
                method_name = { $method_name },
                lifetimes_parameters = {  $($($lifetime,)*)?  },
                arguments = {  $($(  $arg_name: $arg_ty,  )*)?  $($(  $arg_name_adv: $arg_ty_adv,  )*)?  },
                return_type = {  $($return_type)?  },
                self_ident = { self },
                self_type = {
                    $(  $(&$($self_life)?)? $(mut $(;;; $self_is_mut_ref)?)? self  )?
                    $(  $(&$($self_life_adv)?)? $(mut $(;;; $self_is_mut_ref_adv)?)?  self  )?
                },
                signature = {
                    // Method definition:
                    $(unsafe $(;;; $is_unsafe)?)? fn $method_name
                    // Lifetime Parameters:
                    $(<
                        $( $lifetime $(: $lifetime_bound)? ),*
                    >)?
                    // parameters with shorthand self (&self, &mut self, self)
                    $((
                        $(&$($self_life)?)? $(mut $(;;; $self_is_mut_ref)?)? self $(,$arg_name: $arg_ty)*,
                    ))?
                    // parameters with long typed self (self: &Self, self: &mut Self, self: Self)
                    $((
                        $(mut $(;;; $self_is_mut_binding)?)? self: $(&$($self_life_adv)?)? $(mut $(;;; $self_is_mut_ref_adv:ident)?)? Self $(,$arg_name_adv: $arg_ty_adv)*,
                    ))?
                    // Return type
                    $(-> $return_type)?
                    // Where clause (this is a compiler warning for "object safe" methods, methods without `where Self: Sized`):
                    $(where
                        $( $where_clause_ty
                            $(: $where_clause_bound)?
                            $(: ?$where_clause_unsized_bound)?
                            $(: $where_clause_lifetime_bound)?
                        ),*
                    )?
                },
            }},
            parsed_associated_types = $parsed_types,
        }
    };
    ////////////////////////////////////////////////////////////////////////////////
    // Parse: trait method with default implementation
    ////////////////////////////////////////////////////////////////////////////////
    (@parse_items
        trait_def = $trait_def:tt,
        unparsed_items = {
            // Attributes:
            $(#[$attr:meta])*
            // Method definition:
            $(unsafe $(;;; $is_unsafe:ident)?)? fn $method_name:ident
            // Lifetime Parameters:
            $(<
                $( $lifetime:lifetime $(: $lifetime_bound:lifetime)? ),* $(,)?
            >)?
            // parameters with shorthand self (&self, &mut self, self)
            $((
                $(&$($self_life:lifetime)?)? $(mut $(;;; $self_is_mut_ref:ident)?)? self $(,$arg_name:ident: $arg_ty:ty)* $(,)?
            ))?
            // parameters with long typed self (self: &Self, self: &mut Self, self: Self)
            $((
                $(mut $(;;; $self_is_mut_binding:ident)?)? self: $(&$($self_life_adv:lifetime)?)? $(mut $(;;; $self_is_mut_ref_adv:ident)?)? Self $(,$arg_name_adv:ident: $arg_ty_adv:ty)* $(,)?
            ))?
            // Return type
            $(-> $return_type:ty)?
            // Where clause (this is a compiler warning for "object safe" methods, methods without `where Self: Sized`):
            $(where
                $( $where_clause_ty:ty
                    $(: $where_clause_bound:path)?
                    $(: ?$where_clause_unsized_bound:path)?
                    $(: $where_clause_lifetime_bound:lifetime)?
                ),* $(,)?
            )?
            // Default method implementation:
            { $($default_impl:tt)* }
            // Next trait item:
            $($unparsed_rest:tt)*
        },
        parsed_fns = { $($parsed:tt)* },
        parsed_associated_types = $parsed_types:tt $(,)?
    ) => {
        $crate::__define_v_table_internal! {@parse_items
            trait_def = $trait_def,
            unparsed_items = { $($unparsed_rest)* },
            parsed_fns = { $($parsed)* {
                is_unsafe = {  $(unsafe $(;;; $is_unsafe)?)?  },
                method_name = { $method_name },
                lifetimes_parameters = {  $($($lifetime,)*)?  },
                arguments = {  $($(  $arg_name: $arg_ty,  )*)?  $($(  $arg_name_adv: $arg_ty_adv,  )*)?  },
                return_type = {  $($return_type)?  },
                self_ident = { self },
                self_type = {
                    $(  $(&$($self_life)?)? $(mut $(;;; $self_is_mut_ref)?)? self  )?
                    $(  $(&$($self_life_adv)?)? $(mut $(;;; $self_is_mut_ref_adv)?)?  self  )?
                },
                signature = {
                    // Method definition:
                    $(unsafe $(;;; $is_unsafe)?)? fn $method_name
                    // Lifetime Parameters:
                    $(<
                        $( $lifetime $(: $lifetime_bound)? ),*
                    >)?
                    // parameters with shorthand self (&self, &mut self, self)
                    $((
                        $(&$($self_life)?)? $(mut $(;;; $self_is_mut_ref)?)? self $(,$arg_name: $arg_ty)*,
                    ))?
                    // parameters with long typed self (self: &Self, self: &mut Self, self: Self)
                    $((
                        $(mut $(;;; $self_is_mut_binding)?)? self: $(&$($self_life_adv)?)? $(mut $(;;; $self_is_mut_ref_adv:ident)?)? Self $(,$arg_name_adv: $arg_ty_adv)*,
                    ))?
                    // Return type
                    $(-> $return_type)?
                    // Where clause (this is a compiler warning for "object safe" methods, methods without `where Self: Sized`):
                    $(where
                        $( $where_clause_ty
                            $(: $where_clause_bound)?
                            $(: ?$where_clause_unsized_bound)?
                            $(: $where_clause_lifetime_bound)?
                        ),*
                    )?
                },
            }},
            parsed_associated_types = $parsed_types,
        }
    };
    ////////////////////////////////////////////////////////////////////////////////
    // Parse: associated type
    ////////////////////////////////////////////////////////////////////////////////
    (@parse_items
        trait_def = {
            $(#[$trait_attr:meta])*
            $visibility:vis $(unsafe $(;;; $is_unsafe_trait:ident)?)? trait $trait_name:ident
            $(<
                $( $lifetime:lifetime $(: $lifetime_bound:lifetime)? ,)*
                $( $generics:ident
                    $(: $generics_bound:path)?
                    $(: ?$generics_unsized_bound:path)?
                    $(: $generics_lifetime_bound:lifetime)?
                    $(= $generics_default:ty)?
                ,)*
            >)?
            $(:
                $($(+)? $super_lifetime_bound:lifetime )*
                $($(+)? $super_bound:path )*
            )?
            $(where
                $( $where_clause_ty:ty
                    $(: $where_clause_bound:path)?
                    $(: ?$where_clause_unsized_bound:path)?
                    $(: $where_clause_lifetime_bound:lifetime)?
                ),* $(,)?
            )?
            {}
        },
        unparsed_items = {
            // Attributes:
            $(#[$attr:meta])*
            // Type definition:
            type $name:ident
            // Bounds:
            $(:
                $($(+)? $life_bound:lifetime )*
                $($(+)? $trait_bound:path )*
            )?
            // End token:
            ;
            // Next trait item:
            $($unparsed_rest:tt)*
        },
        parsed_fns = $parsed_fns:tt,
        parsed_associated_types = { $($parsed:tt)* } $(,)?
    ) => {
        $crate::__define_v_table_internal! {@parse_items
            trait_def = {
                $(#[$trait_attr])*
                $visibility $(unsafe $($is_unsafe_trait)?)? trait $trait_name
                $(<
                    $( $lifetime $(: $lifetime_bound)? ,)*
                    $( $generics
                        $(: $generics_bound)?
                        $(: ?$generics_unsized_bound)?
                        $(: $generics_lifetime_bound)?
                        $(= $generics_default)?
                    ,)*
                >)?
                $(:
                    $(+ $super_lifetime_bound )*
                    $(+ $super_bound )*
                )?
                $(where
                    $( $where_clause_ty
                        $(: $where_clause_bound)?
                        $(: ?$where_clause_unsized_bound)?
                        $(: $where_clause_lifetime_bound)?
                    ,)*
                )? {}
            },
            unparsed_items = { $($unparsed_rest)* },
            parsed_fns = $parsed_fns,
            parsed_associated_types = { $($parsed)* {
                name = { $name },
                bounds = {  $(:  $($life_bound, )*  $( $trait_bound, )*  )?  },
                trait_params = {  $(  $($lifetime,)* $($generics,)*  )?  },
            }},
        }
    };
    ////////////////////////////////////////////////////////////////////////////////
    // Parse: base case, all items parsed
    ////////////////////////////////////////////////////////////////////////////////
    (@parse_items
        trait_def = $trait_def:tt,
        unparsed_items = {},
        parsed_fns = $parsed_fns:tt,
        parsed_associated_types = $parsed_types:tt $(,)?
    ) => {
        $crate::__define_v_table_internal! {@generate_code
            trait_def = $trait_def,
            parsed_fns = $parsed_fns,
            parsed_associated_types = $parsed_types,
        }
    };
    ////////////////////////////////////////////////////////////////////////////////
    // Generates all vtable code once parsing is finished
    ////////////////////////////////////////////////////////////////////////////////
    (@generate_code
        trait_def = {
            $(#[$trait_attr:meta])*
            $visibility:vis $(unsafe $(;;; $is_unsafe_trait:ident)?)? trait $trait_name:ident
            $(<
                $( $lifetime:lifetime $(: $lifetime_bound:lifetime)? ,)*
                $( $generics:ident
                    $(: $generics_bound:path)?
                    $(: ?$generics_unsized_bound:path)?
                    $(: $generics_lifetime_bound:lifetime)?
                    $(= $generics_default:ty)?
                ,)*
            >)?
            $(:
                $($(+)? $super_lifetime_bound:lifetime )*
                $($(+)? $super_bound:path )*
            )?
            $(where
                $( $where_clause_ty:ty
                    $(: $where_clause_bound:path)?
                    $(: ?$where_clause_unsized_bound:path)?
                    $(: $where_clause_lifetime_bound:lifetime)?
                ),* $(,)?
            )?
            {}
        },
        parsed_fns = { $({
            is_unsafe = {  $(unsafe $(;;; $method_is_unsafe:ident)?)?  },
            method_name = { $method_name:ident },
            lifetimes_parameters = { $($method_lifetime_parameter:lifetime),* $(,)? },
            arguments = { $(  $method_arg_name:ident: $method_arg_ty:ty,  )* },
            return_type = {  $($return_type:ty)?  },
            self_ident = {  $method_self_ident:ident  },
            self_type = {
                $( &  $(;;;$method_is_ref:ident)?  $($method_self_life:lifetime)? )? $(mut $(;;; $method_self_is_mut_ref:ident)?)? self
            },
            signature = {  $($method_signature:tt)*  },
        })* },
        parsed_associated_types = { $({
            name = { $associated_type_name:ident },
            bounds = {  $(:  $($associated_type_life_bound:lifetime,)*  $( $associated_type_trait_bound:path,)*  )?  },
            trait_params = {  $($associated_type_trait_lifetime:lifetime,)* $($associated_type_trait_generics:ident,)*  },
        })* } $(,)?
    ) => {
        // Have parsed all trait items!
        const _: fn() = || {
            // VTable type definition:
            #[allow(explicit_outlives_requirements)]
            $visibility struct __VTable
            <
                $(
                    $( $lifetime $(: $lifetime_bound)? ,)*
                    $( $generics
                        $(: $generics_bound)?
                        $(: ?$generics_unsized_bound)?
                        $(: $generics_lifetime_bound)?
                    ,)*
                )?
                $($associated_type_name $(: $($associated_type_life_bound+)*  $( $associated_type_trait_bound+)*)? ,)*
                __CommonData,
            >
            where
                Self: $($($super_lifetime_bound +)* $($super_bound +)*)? ::core::marker::Sized,
            $(
                $( $where_clause_ty:ty
                    $(: $where_clause_bound:path)?
                    $(: ?$where_clause_unsized_bound:path)?
                    $(: $where_clause_lifetime_bound:lifetime)?
                ),*
            )?
            {
                $(
                    $method_name: for<'__this, $( $method_lifetime_parameter ),* > fn(
                        // Self type:
                        $crate::__define_v_table_internal!{@if ($(true$(;;;$method_is_ref:ident)?)?false)
                            // Self is a reference:
                            {$(&  $($method_self_life)? )?  $(mut $(;;; $method_self_is_mut_ref)?)?  $crate::RawThin<'__this, Self, $crate::Split<__CommonData>, $crate::auto_traits::NoAutoTraits, ()>}
                            else
                            // Self is taken by value:
                            {$crate::RawThinBox<'__this, Self, $crate::Taken<__CommonData>, $crate::auto_traits::NoAutoTraits, ()>}
                        },
                        // Args:
                        $($method_arg_ty),*
                    ) $(-> $return_type)?,
                )*
                __drop: for<'__this> fn($crate::RawThinBox<'__this, Self,  $crate::Taken<__CommonData>, $crate::auto_traits::NoAutoTraits, ()>),

                // Generic or lifetimes might not be used by methods. This is allowed in traits but not in structs.
                // This marker ensures that the type and lifetime parameters are used without affecting the auto
                // traits that are implemented for the VTable (since fn pointers always implement all auto traits).
                __ensure_all_type_params_are_used: ::core::marker::PhantomData<
                    fn() -> dyn $trait_name
                        <
                            $(  $($lifetime,)* $($generics,)*  )?
                            $($associated_type_name = $associated_type_name, )*
                        >
                >,

                // Ensure the __VTable can't be constructed elsewhere (the current module could do
                // `<TraitName as ThinTrait>::VTable { method: || (), }` but it can't name the private type).
                __priv: __Private,
            }
            struct __Private;

            // Implement the trait for __VTable so that associated types can be resolved in method argument types:
            #[allow(unused_variables, unused_mut, clippy::needless_arbitrary_self_type)]
            $(unsafe $($is_unsafe_trait)?)? impl
            <
                $(
                    $( $lifetime $(: $lifetime_bound)? ,)*
                    $( $generics
                        $(: $generics_bound)?
                        $(: ?$generics_unsized_bound)?
                        $(: $generics_lifetime_bound)?
                    ,)*
                )?
                $($associated_type_name $(: $($associated_type_life_bound+)*  $( $associated_type_trait_bound+)*)? ,)*
                __CommonData,
            >
            $trait_name<$(  $($lifetime,)* $($generics,)*  )?>
            for
            __VTable<$(  $($lifetime,)* $($generics,)*  )?  $($associated_type_name,)*  __CommonData,>
            $(
            where
                $( $where_clause_ty:ty
                    $(: $where_clause_bound:path)?
                    $(: ?$where_clause_unsized_bound:path)?
                    $(: $where_clause_lifetime_bound:lifetime)?
                ),* $(,)?
            )?
            {
                $(
                    type $associated_type_name = $associated_type_name;
                )*
                $(
                    $($method_signature)*
                    {
                        // We only implement the trait to support naming associated types.
                        // So no reason to provide a method implementation.
                        ::core::unimplemented!()
                    }
                )*
            }


            // impl `EnforceAutoTraits` for __VTable:
            // Used to check that a type `__T` implements the auto traits guaranteed
            // by the `VTableEnforcedAutoTraits` implementation.
            // This is used to catch unsafe implementations of the `ThinTrait` trait
            // and to catch some unsafe constructions of the `$crate::VTable` type
            // at compile time.
            unsafe impl
            <
                $(
                    $( $lifetime $(: $lifetime_bound)? ,)*
                    $( $generics
                        $(: $generics_bound)?
                        $(: ?$generics_unsized_bound)?
                        $(: $generics_lifetime_bound)?
                    ,)*
                )?
                $($associated_type_name $(: $($associated_type_life_bound+)*  $( $associated_type_trait_bound+)*)? ,)*
                __CommonData,
                __T,
            >
            $crate::auto_traits::EnforceAutoTraits<__T> for __VTable<$(  $($lifetime,)* $($generics,)*  )?  $($associated_type_name,)*  __CommonData>
            where
                __T: $trait_name<$(  $($lifetime,)* $($generics,)*  )?  $($associated_type_name = $associated_type_name,)*  > + ?::core::marker::Sized,
            $(
                $( $where_clause_ty:ty
                    $(: $where_clause_bound:path)?
                    $(: ?$where_clause_unsized_bound:path)?
                    $(: $where_clause_lifetime_bound:lifetime)?
                ),* $(,)?
            )?
            {}


            // impl `VTableEnforcedAutoTraits` for __VTable:
            // Used to implement some auto traits for Thin<dyn Trait> that are guaranteed by
            // the trait (for example using `Send` as a supertrait).
            impl
            <
                $(
                    $( $lifetime $(: $lifetime_bound)? ,)*
                    $( $generics
                        $(: $generics_bound)?
                        $(: ?$generics_unsized_bound)?
                        $(: $generics_lifetime_bound)?
                    ,)*
                )?
                $($associated_type_name $(: $($associated_type_life_bound+)*  $( $associated_type_trait_bound+)*)? ,)*
                __CommonData,
            >
            $crate::auto_traits::VTableEnforcedAutoTraits
            for
            __VTable<$(  $($lifetime,)* $($generics,)*  )?  $($associated_type_name,)*  __CommonData,>
            $(
            where
                $( $where_clause_ty:ty
                    $(: $where_clause_bound:path)?
                    $(: ?$where_clause_unsized_bound:path)?
                    $(: $where_clause_lifetime_bound:lifetime)?
                ),* $(,)?
            )?
            {
                type UncheckedAutoTraitMarker = dyn $trait_name
                <
                    $(  $($lifetime,)* $($generics,)*  )?
                    $($associated_type_name = $associated_type_name, )*
                >;
            }

            // impl `GetThinTraitVTable` for all types that implement the trait:
            impl
            <
                $(
                    $( $lifetime $(: $lifetime_bound)? ,)*
                    $( $generics
                        $(: $generics_bound)?
                        $(: ?$generics_unsized_bound)?
                        $(: $generics_lifetime_bound)?
                    ,)*
                )?
                $($associated_type_name $(: $($associated_type_life_bound+)*  $( $associated_type_trait_bound+)*)? ,)*
                __CommonData,
                __T,
            >
            $crate::GetThinTraitVTable<__T>
            for __VTable<$(  $($lifetime,)* $($generics,)*  )?  $($associated_type_name,)*  __CommonData>
            where
                __T: $trait_name<$(  $($lifetime,)* $($generics,)*  )?  $($associated_type_name = $associated_type_name,)*  >,
            $(
                $( $where_clause_ty:ty
                    $(: $where_clause_bound:path)?
                    $(: ?$where_clause_unsized_bound:path)?
                    $(: $where_clause_lifetime_bound:lifetime)?
                ),* $(,)?
            )?
            {
                // This warning can happen if a method is unsafe (then any unsafe uses inside it becomes unnecessary).
                #[allow(unused_unsafe, clippy::needless_arbitrary_self_type)]
                fn get_vtable() -> $crate::VTable<Self, __T> {
                    let get_vtable = || -> &_ {
                        let vtable: &__VTable<$(  $($lifetime,)* $($generics,)*  )?  $($associated_type_name,)*  __CommonData> = $crate::__define_v_table_internal! {
                            @create_vtable
                            parsed_fns = {
                                $({
                                    is_unsafe = {  $(unsafe $(;;; $method_is_unsafe:ident)?)?  },
                                    method_name = { $method_name },
                                    lifetimes_parameters = { $($method_lifetime_parameter,)* },
                                    arguments = { $(  $method_arg_name: $method_arg_ty,  )* },
                                    return_type = {  $($return_type)?  },
                                    self_ident = {  $method_self_ident  },
                                    self_type = {
                                        $( &$($method_self_life)? )? $(mut $(;;; $method_self_is_mut_ref)?)? self
                                    },
                                    signature = {  $($method_signature)*  },
                                })*
                            },
                            vtable_methods = {},
                            common_info = {
                                trait_name = $trait_name,
                                trait_lifetime = $($($lifetime),*)?,
                                trait_generics = $($($generics),*)?,
                                // Don't need associated types since
                                // `<Self as Trait<generics>>::method` is already fully
                                // resolved (associated types are deduced from self type).
                            },
                            vtable_info = {
                                erased_type = __T,
                                vtable_name = __VTable,
                            },
                        };
                        // We are returning a reference to a local variable from a function. If the
                        // borrow checker allows this then it should be because the vtable was
                        // promoted to a `'static` reference.
                        vtable
                    };
                    let vtable = get_vtable();
                    // Safety: this vtable was specifically constructed for the type `__T` so
                    // it will behave sensibly for that type. Our vtable also only contains
                    // methods and all of them take the type `__T` as inputs so the vtable
                    // can't outlive any lifetime requirements of the type `__T`.
                    unsafe { $crate::VTable::new(vtable) }
                }
            }

            // impl `VTableDrop` for __VTable:
            // Allows the vtable to be used in ThinBox's Drop implementation.
            impl
            <
                $(
                    $( $lifetime $(: $lifetime_bound)? ,)*
                    $( $generics
                        $(: $generics_bound)?
                        $(: ?$generics_unsized_bound)?
                        $(: $generics_lifetime_bound)?
                    ,)*
                )?
                $($associated_type_name $(: $($associated_type_life_bound+)*  $( $associated_type_trait_bound+)*)? ,)*
                __CommonData,
            >
            $crate::VTableDrop<__CommonData>
            for
            __VTable<$(  $($lifetime,)* $($generics,)*  )?  $($associated_type_name,)*  __CommonData,>
            $(
            where
                $( $where_clause_ty:ty
                    $(: $where_clause_bound:path)?
                    $(: ?$where_clause_unsized_bound:path)?
                    $(: $where_clause_lifetime_bound:lifetime)?
                ),* $(,)?
            )?
            {
                unsafe fn drop_erased_box(&self, erased_box: $crate::RawThinBox<'_, Self, $crate::Taken<__CommonData>, $crate::auto_traits::NoAutoTraits, ()>) {
                    // Forward the call to the vtable drop method:
                    (self.__drop)(erased_box)
                }
            }

            // impl the user's trait for `ThinWithoutCommon` so that the trait methods can be called
            // for references to the thin trait object.
            #[allow(unused_mut, clippy::needless_arbitrary_self_type)]
            // This warning can happen if a method is unsafe (then any unsafe uses becomes unnecessary).
            #[allow(unused_unsafe)]
            $(unsafe $($is_unsafe_trait)?)? impl
            <
                $(
                    $( $lifetime $(: $lifetime_bound)? ,)*
                    $( $generics
                        $(: $generics_bound)?
                        $(: ?$generics_unsized_bound)?
                        $(: $generics_lifetime_bound)?
                    ,)*
                )?
                $($associated_type_name $(: $($associated_type_life_bound+)*  $( $associated_type_trait_bound+)*)? ,)*
                __CommonData,
                __ThinTrait,
            >
            $trait_name<$(  $($lifetime,)* $($generics,)*  )?>
            for
            $crate::ThinWithoutCommon<'_, __ThinTrait, __CommonData>
            where
                // Ensure all required auto traits are implemented (might for example constrain __CommonData):
                Self: $($($super_lifetime_bound +)* $($super_bound +)*)?,
                // Ensure the thin trait implementation uses our vtable:
                __ThinTrait: $crate::ThinTrait<
                    __CommonData,
                    VTable =  __VTable<$(  $($lifetime,)* $($generics,)*  )?  $($associated_type_name,)*  __CommonData,>
                > + ?::core::marker::Sized,
                // Ensure auto trait config works for our vtable:
                <
                    __ThinTrait as $crate::ThinTrait<__CommonData>
                >::AutoTraitConfig: $crate::auto_traits::AutoTraitConfig<
                    <
                        __VTable<$(  $($lifetime,)* $($generics,)*  )?  $($associated_type_name,)*  __CommonData,>
                        as $crate::auto_traits::VTableEnforcedAutoTraits
                    >::UncheckedAutoTraitMarker
                >,
                 __VTable<$(  $($lifetime,)* $($generics,)*  )?  $($associated_type_name,)*  __CommonData,>: $($($super_lifetime_bound +)*)?,
            $(
                $( $where_clause_ty:ty
                    $(: $where_clause_bound:path)?
                    $(: ?$where_clause_unsized_bound:path)?
                    $(: $where_clause_lifetime_bound:lifetime)?
                ),* $(,)?
            )?
            {
                $(
                    type $associated_type_name = $associated_type_name;
                )*
                $(
                    $($method_signature)*
                    {
                        $crate::__define_v_table_internal!{@if ($(true$(;;;$method_is_ref:ident)?)?false)
                            // Self is a reference:
                            {
                                // Safety: we will only call vtable methods is sensible ways.
                                let __vtable = unsafe { $crate::ThinWithoutCommon::get_vtable(&$method_self_ident) };

                                let __erased_thin = $crate::__define_v_table_internal!{@if ($(true$(;;;$method_self_is_mut_ref)?)?false)
                                    // Self is a mutable reference:
                                    {{
                                        $crate::RawThin::without_lifetime_marker_mut(
                                            $crate::RawThin::as_weaker_auto_traits_marker_mut(
                                                $crate::ThinWithoutCommon::as_raw_mut($method_self_ident)
                                            )
                                        )
                                    }}
                                    else
                                    // Self is an immutable reference:
                                    {{
                                        $crate::RawThin::without_lifetime_marker(
                                            $crate::RawThin::as_weaker_auto_traits_marker(
                                                $crate::ThinWithoutCommon::as_raw($method_self_ident)
                                            )
                                        )
                                    }}
                                };

                                let result = (__vtable.$method_name)(__erased_thin, $($method_arg_name),* );

                                $crate::__define_v_table_internal!{@transmute_return_type
                                    self_lifetime = { $($($method_self_life)?)? },
                                    return_type = { $($return_type)? },
                                    value_to_transmute = {result},
                                }
                            }
                            else
                            // Self is taken by value:
                            {::core::unreachable!("`ThinWithoutCommon` is always behind a reference.")}
                        }
                    }
                )*
            }

            // impl the user's trait for `ThinBox` so that the trait methods can be called
            // for methods that consumes self.
            #[allow(unused_mut, clippy::needless_arbitrary_self_type)]
            // This warning can happen if a method is unsafe (then any unsafe uses becomes unnecessary).
            #[allow(unused_unsafe)]
            $(unsafe $($is_unsafe_trait)?)? impl
            <
                $(
                    $( $lifetime $(: $lifetime_bound)? ,)*
                    $( $generics
                        $(: $generics_bound)?
                        $(: ?$generics_unsized_bound)?
                        $(: $generics_lifetime_bound)?
                    ,)*
                )?
                $($associated_type_name $(: $($associated_type_life_bound+)*  $( $associated_type_trait_bound+)*)? ,)*
                __CommonData,
                __ThinTrait,
            >
            $trait_name<$(  $($lifetime,)* $($generics,)*  )?>
            for
            $crate::ThinBox<'_, __ThinTrait, __CommonData>
            where
                // Ensure all required auto traits are implemented (might for example constrain __CommonData):
                Self: $($($super_lifetime_bound +)* $($super_bound +)*)?,
                // Ensure the thin trait implementation uses our vtable:
                __ThinTrait: $crate::ThinTrait<
                    __CommonData,
                    VTable =  __VTable<$(  $($lifetime,)* $($generics,)*  )?  $($associated_type_name,)*  __CommonData,>
                > + ?::core::marker::Sized,
                // Ensure auto trait config works for our vtable:
                <
                    __ThinTrait as $crate::ThinTrait<__CommonData>
                >::AutoTraitConfig: $crate::auto_traits::AutoTraitConfig<
                    <
                        __VTable<$(  $($lifetime,)* $($generics,)*  )?  $($associated_type_name,)*  __CommonData,>
                        as $crate::auto_traits::VTableEnforcedAutoTraits
                    >::UncheckedAutoTraitMarker
                >,
                 __VTable<$(  $($lifetime,)* $($generics,)*  )?  $($associated_type_name,)*  __CommonData,>: $($($super_lifetime_bound +)*)?,
            $(
                $( $where_clause_ty:ty
                    $(: $where_clause_bound:path)?
                    $(: ?$where_clause_unsized_bound:path)?
                    $(: $where_clause_lifetime_bound:lifetime)?
                ),* $(,)?
            )?
            {
                $(
                    type $associated_type_name = $associated_type_name;
                )*
                $(
                    $($method_signature)*
                    {
                        // Safety: we will only call vtable methods is sensible ways.
                        let __vtable = unsafe { $crate::ThinWithoutCommon::get_vtable(&$method_self_ident) };

                        let __erased = $crate::__define_v_table_internal!{@if ($(true$(;;;$method_is_ref:ident)?)?false)
                            // Self is a reference:
                            {
                                $crate::__define_v_table_internal!{@if ($(true$(;;;$method_self_is_mut_ref)?)?false)
                                    // Self is a mutable reference:
                                    {{
                                        $crate::RawThin::without_lifetime_marker_mut(
                                            $crate::RawThin::as_weaker_auto_traits_marker_mut(
                                                $crate::ThinWithoutCommon::as_raw_mut($method_self_ident)
                                            )
                                        )
                                    }}
                                    else
                                    // Self is an immutable reference:
                                    {{
                                        $crate::RawThin::without_lifetime_marker(
                                            $crate::RawThin::as_weaker_auto_traits_marker(
                                                $crate::ThinWithoutCommon::as_raw($method_self_ident)
                                            )
                                        )
                                    }}
                                }
                            }
                            else
                            // Self is taken by value:
                            {
                                unsafe {
                                    // Safety: we can forget the lifetime since we will be consuming
                                    // the value now anyway.
                                    $crate::ThinBox::into_raw($method_self_ident)
                                        .weaken_auto_traits_marker()
                                        .forget_lifetime()
                                        .free_common_data()
                                }
                            }
                        };
                        let result = (__vtable.$method_name)(__erased, $($method_arg_name),* );

                        $crate::__define_v_table_internal!{@transmute_return_type
                            self_lifetime = { $($($method_self_life)?)? },
                            return_type = { $($return_type)? },
                            value_to_transmute = {result},
                        }
                    }
                )*
            }

            // impl the user's trait for `ThinBoxWithoutCommon` so that the trait methods can be called
            // for methods that consumes self.
            #[allow(unused_mut, clippy::needless_arbitrary_self_type)]
            // This warning can happen if a method is unsafe (then any unsafe uses becomes unnecessary).
            #[allow(unused_unsafe)]
            $(unsafe $($is_unsafe_trait)?)? impl
            <
                $(
                    $( $lifetime $(: $lifetime_bound)? ,)*
                    $( $generics
                        $(: $generics_bound)?
                        $(: ?$generics_unsized_bound)?
                        $(: $generics_lifetime_bound)?
                    ,)*
                )?
                $($associated_type_name $(: $($associated_type_life_bound+)*  $( $associated_type_trait_bound+)*)? ,)*
                __CommonData,
                __ThinTrait,
            >
            $trait_name<$(  $($lifetime,)* $($generics,)*  )?>
            for
            $crate::ThinBoxWithoutCommon<'_, __ThinTrait, __CommonData>
            where
                // Ensure all required auto traits are implemented (might for example constrain __CommonData):
                Self: $($($super_lifetime_bound +)* $($super_bound +)*)?,
                // Ensure the thin trait implementation uses our vtable:
                __ThinTrait: $crate::ThinTrait<
                    __CommonData,
                    VTable =  __VTable<$(  $($lifetime,)* $($generics,)*  )?  $($associated_type_name,)*  __CommonData,>
                > + ?::core::marker::Sized,
                // Ensure auto trait config works for our vtable:
                <
                    __ThinTrait as $crate::ThinTrait<__CommonData>
                >::AutoTraitConfig: $crate::auto_traits::AutoTraitConfig<
                    <
                        __VTable<$(  $($lifetime,)* $($generics,)*  )?  $($associated_type_name,)*  __CommonData,>
                        as $crate::auto_traits::VTableEnforcedAutoTraits
                    >::UncheckedAutoTraitMarker
                >,
                 __VTable<$(  $($lifetime,)* $($generics,)*  )?  $($associated_type_name,)*  __CommonData,>: $($($super_lifetime_bound +)*)?,
            $(
                $( $where_clause_ty:ty
                    $(: $where_clause_bound:path)?
                    $(: ?$where_clause_unsized_bound:path)?
                    $(: $where_clause_lifetime_bound:lifetime)?
                ),* $(,)?
            )?
            {
                $(
                    type $associated_type_name = $associated_type_name;
                )*
                $(
                    $($method_signature)*
                    {
                        // Safety: we will only call vtable methods is sensible ways.
                        let __vtable = unsafe { $crate::ThinWithoutCommon::get_vtable(&$method_self_ident) };

                        let __erased = $crate::__define_v_table_internal!{@if ($(true$(;;;$method_is_ref:ident)?)?false)
                            // Self is a reference:
                            {
                                $crate::__define_v_table_internal!{@if ($(true$(;;;$method_self_is_mut_ref)?)?false)
                                    // Self is a mutable reference:
                                    {{
                                        $crate::RawThin::without_lifetime_marker_mut(
                                            $crate::RawThin::as_weaker_auto_traits_marker_mut(
                                                $crate::ThinWithoutCommon::as_raw_mut($method_self_ident)
                                            )
                                        )
                                    }}
                                    else
                                    // Self is an immutable reference:
                                    {{
                                        $crate::RawThin::without_lifetime_marker(
                                            $crate::RawThin::as_weaker_auto_traits_marker(
                                                $crate::ThinWithoutCommon::as_raw($method_self_ident)
                                            )
                                        )
                                    }}
                                }
                            }
                            else
                            // Self is taken by value:
                            {
                                unsafe {
                                    // Safety: we can forget the lifetime since we will be consuming
                                    // the value now anyway.
                                    $crate::ThinBoxWithoutCommon::into_raw($method_self_ident)
                                        .weaken_auto_traits_marker()
                                        .forget_lifetime()
                                }
                            }
                        };
                        let result = (__vtable.$method_name)(__erased, $($method_arg_name),* );

                        $crate::__define_v_table_internal!{@transmute_return_type
                            self_lifetime = { $($($method_self_life)?)? },
                            return_type = { $($return_type)? },
                            value_to_transmute = {result},
                        }
                    }
                )*
            }

            // impl `ThinTrait` for `dyn UserTrait` as a way to name the anonymous vtable type:
            // We also implement `ThinTrait` for auto trait combinations like: `dyn UserTrait + Send + Sync`.
            // (we use a macro for that, since all the implementations are largely the same.)
            $crate::__define_v_table_internal! {@thin_trait_impl
                auto_trait_combinations = {
                    // No auto traits:
                    => (),

                    // Send:
                    ::core::marker::Send => $crate::auto_traits::HasSend<()>,
                    // Sync:
                    ::core::marker::Sync => $crate::auto_traits::HasSync<()>,
                    // Send + Sync:
                    ::core::marker::Send + ::core::marker::Sync => $crate::auto_traits::HasSend<$crate::auto_traits::HasSync<()>>,

                    // Unpin:
                    ::core::marker::Unpin => $crate::auto_traits::HasUnpin<()>,
                    // Send + Unpin:
                    ::core::marker::Send + ::core::marker::Unpin => $crate::auto_traits::HasSend<$crate::auto_traits::HasUnpin<()>>,
                    // Sync + Unpin:
                    ::core::marker::Sync + ::core::marker::Unpin => $crate::auto_traits::HasSync<$crate::auto_traits::HasUnpin<()>>,
                    // Send + Sync + Unpin:
                    ::core::marker::Send + ::core::marker::Sync + ::core::marker::Unpin => $crate::auto_traits::HasSend<$crate::auto_traits::HasSync<$crate::auto_traits::HasUnpin<()>>>,
                },
                // Everything before the `for` in `impl Trait for Type {...}`:
                before_for = {
                    impl
                    <
                        '__a,
                        $(
                            $( $lifetime $(: $lifetime_bound)? ,)*
                            $( $generics
                                $(: $generics_bound)?
                                $(: ?$generics_unsized_bound)?
                                $(: $generics_lifetime_bound)?
                            ,)*
                        )?
                        $($associated_type_name $(: $($associated_type_life_bound+)*  $( $associated_type_trait_bound+)*)? ,)*
                        __CommonData,
                    >
                    $crate::ThinTrait<__CommonData>
                },
                // `dyn Trait` syntax for the user specified trait (without any auto traits):
                dyn_trait_base = {
                    dyn $trait_name
                    <
                        $(  $($lifetime,)* $($generics,)*  )?
                        $($associated_type_name = $associated_type_name, )*
                    >
                    + '__a
                },
                // Where clause (`impl Trait for Type [insert where clause here] {...}`):
                where_clause = {
                    where
                        __VTable<$(  $($lifetime,)* $($generics,)*  )?  $($associated_type_name,)*  __CommonData>: $($($lifetime +)*)? '__a + ::core::marker::Unpin,
                        $($($lifetime: '__a,)*)?
                    $(
                        $( $where_clause_ty:ty
                            $(: $where_clause_bound:path)?
                            $(: ?$where_clause_unsized_bound:path)?
                            $(: $where_clause_lifetime_bound:lifetime)?
                        ),* $(,)?
                    )?
                },
                // The part of the `ThinTrait` implementation that doesn't depend on auto traits:
                common_impl = {
                    type VTable = __VTable<$(  $($lifetime,)* $($generics,)*  )?  $($associated_type_name,)*  __CommonData>;
                },
            }
        };
    };
    ////////////////////////////////////////////////////////////////////////////////
    // Code that creates the vtable for a specific type:
    ////////////////////////////////////////////////////////////////////////////////
    (@create_vtable
        parsed_fns = { {
            is_unsafe = {  $(unsafe $(;;; $method_is_unsafe:ident)?)?  },
            method_name = { $method_name:ident },
            lifetimes_parameters = { $($method_lifetime_parameter:lifetime),* $(,)? },
            arguments = { $(  $method_arg_name:ident: $method_arg_ty:ty,  )* },
            return_type = {  $($return_type:ty)?  },
            self_ident = {  $method_self_ident:ident  },
            self_type = {
                $( &  $(;;;$method_is_ref:ident)?  $($method_self_life:lifetime)? )? $(mut $(;;; $method_self_is_mut_ref:ident)?)? self
            },
            signature = {  $($method_signature:tt)*  },
        } $($next_fn:tt)* },
        vtable_methods = {  $($vtable_methods:tt)*  },
        common_info = {
            trait_name = $trait_name:ident,
            trait_lifetime = $($trait_lifetime:lifetime),*,
            trait_generics = $($trait_generics:ident),*,
        },
        vtable_info = {
            erased_type = $erased_ty:ident,
            vtable_name = $vtable_name:ident,
        },
    ) => {
        $crate::__define_v_table_internal! {@create_vtable
            parsed_fns = {  $($next_fn)*  },
            vtable_methods = { $($vtable_methods)* {
                $method_name: |__this, $($method_arg_name),*| {
                    $(unsafe $(;;; $method_is_unsafe:ident)?)?  {
                        let result = <$erased_ty as $trait_name<  $($trait_lifetime,)*  $($trait_generics,)*  >>::$method_name(
                            {
                                // Safety: getting access to a vtable requires calling an unsafe
                                // method on a `ThinWithoutCommon` type. This ensures that the
                                // vtable method is only called with a type that has the
                                // correct erased type.
                                $crate::__define_v_table_internal!{@if ($(true$(;;;$method_is_ref:ident)?)?false)
                                    // Self is a reference:
                                    {
                                        $crate::__define_v_table_internal!{@if ($(true$(;;;$method_self_is_mut_ref)?)?false)
                                            // Self is a mutable reference:
                                            {{
                                                let __this = $crate::RawThin::without_lifetime_marker_mut(__this);
                                                let unerased = unsafe { $crate::RawThin::as_unerase_mut::<$erased_ty>(__this) };
                                                $crate::RawThin::as_object_mut(unerased)
                                            }}
                                            else
                                            // Self is an immutable reference:
                                            {{
                                                let __this = $crate::RawThin::without_lifetime_marker(__this);
                                                let unerased = unsafe { $crate::RawThin::as_unerase::<$erased_ty>(__this) };
                                                $crate::RawThin::as_object(unerased)
                                            }}
                                        }
                                    }
                                    else
                                    // Self is taken by value:
                                    {
                                        unsafe { __this.unerase::<$erased_ty>() }.into_inner()
                                    }
                                }
                            }
                            $(,$method_arg_name)*
                        );
                        $crate::__define_v_table_internal!{@transmute_return_type
                            self_lifetime = { $($($method_self_life)?)? },
                            return_type = { $($return_type)? },
                            value_to_transmute = {result},
                        }
                    }
                }
            }},
            common_info = {
                trait_name = $trait_name,
                trait_lifetime = $($trait_lifetime),*,
                trait_generics = $($trait_generics),*,
            },
            vtable_info = {
                erased_type = $erased_ty,
                vtable_name = $vtable_name,
            },
        }
    };
    ////////////////////////////////////////////////////////////////////////////////
    // Code that creates the vtable for a specific type (base case):
    ////////////////////////////////////////////////////////////////////////////////
    (@create_vtable
        parsed_fns = {  },
        vtable_methods = { $({
            $method_name:ident: $method_value:expr
        })* },
        common_info = $common_info:tt,
        vtable_info = {
            erased_type = $erased_ty:ident,
            vtable_name = $vtable_name:ident,
        },
    ) => {
        &__VTable {
            __priv: __Private,
            __ensure_all_type_params_are_used: ::core::marker::PhantomData,
            __drop: |erased| {
                // Safety: this vtable method is only called with `ThinBox`s that
                // contain the type `__T`.
                unsafe { erased.unerase::<__T>().free() };
            },
            $(
                $method_name: $method_value,
            )*
        }
    };
    ////////////////////////////////////////////////////////////////////////////////
    // Code that implements the `ThinTrait` for `dyn UserTrait`:
    ////////////////////////////////////////////////////////////////////////////////
    (@thin_trait_impl
        auto_trait_combinations = {
            $($(+)? $auto_trait:path)* => $config:ty,
            $($next_config:tt)*
        },
        before_for = { $($before_for:tt)* },
        dyn_trait_base = { $($dyn_trait_base:tt)* },
        where_clause = { $($where_clause:tt)* },
        common_impl = { $($common_impl:tt)* },
    ) => {
        $($before_for)*
        for
        (
            $($dyn_trait_base)* $(+ $auto_trait)*
        )
        $($where_clause)*
        {
            $($common_impl)*
            type AutoTraitConfig = $config;
        }


        $crate::__define_v_table_internal!{@thin_trait_impl
            auto_trait_combinations = {
                  $($next_config)*
            },
            before_for = { $($before_for)* },
            dyn_trait_base = { $($dyn_trait_base)* },
            where_clause = { $($where_clause)* },
            common_impl = { $($common_impl)* },
        }
    };
    ////////////////////////////////////////////////////////////////////////////////
    // Code that implements the `ThinTrait` for `dyn UserTrait` (Base case):
    ////////////////////////////////////////////////////////////////////////////////
    (@thin_trait_impl
        auto_trait_combinations = {},
        before_for = { $($before_for:tt)* },
        dyn_trait_base = { $($dyn_trait_base:tt)* },
        where_clause = { $($where_clause:tt)* },
        common_impl = { $($common_impl:tt)* },
    ) => {};
    ////////////////////////////////////////////////////////////////////////////////
    // Transmute a return type to disconnect its lifetime requirement from generic
    // types that shouldn't affect it:
    ////////////////////////////////////////////////////////////////////////////////
    (@transmute_return_type
        self_lifetime = { $($self_lifetime:lifetime)? },
        return_type = { $($return_type:ty)? },
        value_to_transmute = {$result:ident},
    ) => {
        $crate::__define_v_table_internal!{@if ($(true;;;$self_lifetime)?false)
            // Self type has explicit lifetime
            {
                $crate::__define_v_table_internal!{@if ($(true;;;$return_type)?false)
                    // Method has a return type
                    {
                        // If the lifetime `'a` in `&'a $(mut)? self` is tied to the return
                        // type then we need to transmute that lifetime to a new lifetime
                        // that is longer since the compiler will complain that `__T` might
                        // not live long enough for the lifetime to be valid. We know it must
                        // be though since the self type (`Thin`) can't live longer than `__T`.

                        struct __MaybeTransmuteLifetime<T>(T);
                        impl __MaybeTransmuteLifetime<[(); 0]> {
                            unsafe fn handle<T>(v: T) -> T { v }
                        }
                        impl __MaybeTransmuteLifetime<[(); 1]> {
                            unsafe fn handle<T, U>(v: T) -> U {
                                let v = ::core::mem::ManuallyDrop::<T>::new(v);
                                ::core::mem::transmute_copy::<T, U>(&v)
                            }
                        }
                        // Only transmute if the return type is connected to the self lifetime
                        // We could always transmute but this should hopefully be catch limit
                        // the unsafety.
                        unsafe { __MaybeTransmuteLifetime::<[(); {
                            let return_type_text = ::core::stringify!($($return_type)?).as_bytes();
                            let self_lifetime = ::core::stringify!($($self_lifetime)?).as_bytes();

                            let mut scan_index = 0;
                            let mut compare_index = 0;
                            loop {
                                // Uncomment the next line and use the lifetime `'z` as `&'z self` to
                                // test what would happen if transmute wasn't used to fix issues.
                                // if self_lifetime[1] == b"z"[0] { break 0; }

                                if compare_index >= self_lifetime.len() {
                                    // Found self lifetime
                                    break 1;
                                }
                                if compare_index + scan_index >= return_type_text.len() {
                                    // Reached end of return type without finding lifetime
                                    break 0;
                                }
                                if return_type_text[compare_index + scan_index] != self_lifetime[compare_index] {
                                    // Scan next position in return type
                                    scan_index += 1;
                                    compare_index = 0;
                                } else {
                                    // Continue comparing this pos in return type
                                    compare_index += 1;
                                }
                            }
                        }]>::handle($result) }
                    }
                    else
                    // Method returns `()` so self lifetime can't be connected to it:
                    {$result}
                }
            }
            else
            // Self type is not tied to a lifetime.
            {$result}
        }
    };
    ////////////////////////////////////////////////////////////////////////////////
    // Utilities:
    ////////////////////////////////////////////////////////////////////////////////
    (@if (true $($condition:tt)*) { $($true:tt)* } else { $($false:tt)* } ) => { $($true)* };
    (@if ($($condition:tt)*) { $($true:tt)* } else { $($false:tt)* } ) => { $($false)* };
}

/// Parses a trait definition and define a vtable that can be used to interact
/// with a type erased object behind a thin pointer when that type implements
/// the provided trait.
///
/// This macro takes a trait definition as input. If the trait has any supertraits
/// then they should probably be auto traits or traits that have blanket implementations
/// since the macro will try to implement the trait for thin pointer types such as
/// [`ThinBox`] and [`Thin`].
///
/// # Generated code
///
/// This macro will parse the provided trait definition and then expand to the trait
/// as is. The macro will also expand to some extra code that defines a vtable
/// for the trait.
///
/// If the macro is used on a trait like this:
///
/// ```
/// thin_trait_object::define_v_table! {
///     pub trait Number {
///         fn get(&self) -> u32;
///     }
/// }
/// ```
///
/// Then it would expand to some code similar to:
///
/// ```rust,ignore
/// // Emit the trait as it was defined (without any changes):
/// pub trait Number {
///     fn get(&self) -> u32;
/// }
///
/// // Create a new anonymous scope so that the names of the generated items won't
/// // conflict with anything else that might be in scope.
/// const _: fn() = || {
///     // This is the most important part, it defines a struct that holds a function for
///     // each method in the trait and one extra function to drop a type erased `Box`.
///     pub struct __VTable<__CommonData> {
///         get: fn(&thin_trait_object::RawThin<Self, thin_trait_object::Split<__CommonData>, thin_trait_object::auto_traits::NoAutoTraits, ()>),
///         __drop: fn(thin_trait_object::RawThinBox<Self, thin_trait_object::Taken<__CommonData>, thin_trait_object::auto_traits::NoAutoTraits, ()>),
///     }
///
///     // This allows associated types to be named in for arguments or return types of functions inside `__VTable`.
///     impl <__CommonData> Number for __VTable<__CommonData> {
///         fn get(&self) -> u32 { unimplemented!() }
///     }
///     // Enforce that `thin_trait_object::VTable` is only created with types `__T` that implement the `Number` trait.
///     unsafe impl <__CommonData, __T> thin_trait_object::auto_traits::EnforceAutoTraits<__T> for __VTable<__CommonData> where __T: Number {}
///     // Allows `Thin` and other thin pointer types to implement all auto traits that are enforced as supertraits of `Number`.
///     impl <__CommonData> thin_trait_object::auto_traits::VTableEnforcedAutoTraits for __VTable<__CommonData> {
///         type UncheckedAutoTraitMarker = dyn Number;
///     }
///
///     // Create a vtable for a type `__T` that implements `Number`.
///     impl <__CommonData, __T> thin_trait_object::GetThinTraitVTable<__T> for __VTable<__CommonData> where __T: Number {
///         fn get_vtable() -> thin_trait_object::VTable<Self, __T> {
///             // This closure must return a `'static` reference. Static promotion allows this to compile:
///             let get_vtable =|| -> &_ {
///                 let vtable: &__VTable<__CommonData> = &__VTable {
///                     __drop: |erased| { unsafe { erased.unerase::<__T>().free() }; },
///                     get: |__this| {
///                         // Safety: this vtable function will only be called with type `__T`.
///                         let unerased = unsafe { thin_trait_object::RawThin::as_unerase::<__T>(__this) };
///                         <__T as Number>::get(thin_trait_object::RawThin::as_object(unerased))
///                     },
///                 };
///                 vtable
///             };
///             let vtable = get_vtable();
///             // Safety: the vtable will have sensible behavior for `__T`:
///             unsafe { thin_trait_object::VTable::new(vtable) }
///         }
///     }
///     // Allows `ThinBox` to call the vtable drop function in its `Drop` implementation.
///     impl <__CommonData> thin_trait_object::VTableDrop<__CommonData> for __VTable<__CommonData> {
///         unsafe fn drop_erased_box(&self, erased_box: thin_trait_object::RawThinBox<Self, thin_trait_object::Taken<__CommonData>, thin_trait_object::auto_traits::NoAutoTraits, ()>) {
///             (self.__drop)(erased_box)
///         }
///     }
///
///
///     // Implement `Number` for thin pointer types:
///     impl <__CommonData, __ThinTrait> Number for thin_trait_object::ThinWithoutCommon<__ThinTrait, __CommonData>
///     where
///         __ThinTrait: thin_trait_object::ThinTrait<__CommonData, VTable = __VTable<__CommonData>>,
///     {
///         fn get(&self) -> u32 {
///             let __vtable = unsafe { thin_trait_object::ThinWithoutCommon::get_vtable(&self) };
///             let __erased_thin = thin_trait_object::RawThin::as_weaker_auto_traits_marker(thin_trait_object::ThinWithoutCommon::as_raw(self));
///             (__vtable.get)(__erased_thin)
///         }
///     }
///     impl <__CommonData, __ThinTrait> Number for thin_trait_object::ThinBox<__ThinTrait, __CommonData>
///     where
///         __ThinTrait: thin_trait_object::ThinTrait<__CommonData, VTable =__VTable<__CommonData>>,
///     {
///         fn get(&self) -> u32 {
///             let __vtable = unsafe { thin_trait_object::ThinWithoutCommon::get_vtable(&self) };
///             let __erased = thin_trait_object::RawThin::as_weaker_auto_traits_marker(thin_trait_object::ThinWithoutCommon::as_raw(self));
///             (__vtable.get)(__erased)
///         }
///     }
///     impl <__CommonData, __ThinTrait> Number for thin_trait_object::ThinBoxWithoutCommon<__ThinTrait, __CommonData>
///     where
///         __ThinTrait: thin_trait_object::ThinTrait<__CommonData, VTable = __VTable<__CommonData>>,
///     {
///         fn get(&self) -> u32 {
///             let __vtable = unsafe { thin_trait_object::ThinWithoutCommon::get_vtable(&self) };
///             let __erased = thin_trait_object::RawThin::as_weaker_auto_traits_marker(thin_trait_object::ThinWithoutCommon::as_raw(self));
///             (__vtable.get)(__erased)
///         }
///     }
///
///     // Implement `ThinTrait` to allow the `__VTable` type to be named from outside
///     // this anonymous scope.
///     impl <__CommonData> thin_trait_object::ThinTrait<__CommonData> for dyn Number {
///         type VTable = __VTable<__CommonData>;
///         type AutoTraitConfig = ();
///     }
///
///     // Implement `ThinTrait` for some auto trait combinations to allow for easily
///     // enforcing some auto traits for the types that are stored behind a thin pointer.
///     impl <__CommonData> thin_trait_object::ThinTrait<__CommonData> for (dyn Number<> + ::core::marker::Send) {
///         type VTable = __VTable<__CommonData>;
///         type AutoTraitConfig = thin_trait_object::auto_traits::HasSend<()>;
///     }
///     impl <__CommonData> thin_trait_object::ThinTrait<__CommonData> for (dyn Number<> + ::core::marker::Sync) {
///         type VTable = __VTable<__CommonData>;
///         type AutoTraitConfig = thin_trait_object::auto_traits::HasSync<()>;
///     }
///     impl <__CommonData> thin_trait_object::ThinTrait<__CommonData> for (dyn Number<> + ::core::marker::Send + ::core::marker::Sync) {
///         type VTable = __VTable<__CommonData>;
///         type AutoTraitConfig = thin_trait_object::auto_traits::HasSend<thin_trait_object::auto_traits::HasSync<()>>;
///     }
/// };
/// ```
#[macro_export]
macro_rules! define_v_table {
    ($($token:tt)*) => {
        $($token)*
        $crate::__define_v_table_internal! {@input
            $($token)*
        }
    };
}

/// This type guarantees that a vtable is well formed for a specific type.
pub struct VTable<V, T> {
    vtable: StaticVTableRef<V>,
    type_info: PhantomData<fn() -> T>,
}
impl<V, T> fmt::Debug for VTable<V, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VTable").finish()
    }
}
impl<V, T> VTable<V, T>
where
    V: auto_traits::EnforceAutoTraits<T>,
{
    /// Create a new vtable.
    ///
    /// # Safety
    ///
    /// - All methods in the vtable must have sensible behavior when called.
    /// - The type `T` must implement all auto traits that are implemented by the
    /// `<V as auto_traits::VTableEnforcedAutoTraits>::UncheckedAutoTraitMarker`
    /// type. This should always be guaranteed by the `V: auto_traits::EnforceAutoTraits<T>`
    /// trait bound.
    /// - The lifetime for the reference to the vtable type `V` must be long enough
    /// that it is always valid to use with the type `T`. So it can't have a more
    /// restrictive lifetime than `T` itself.
    pub unsafe fn new(vtable: &V) -> Self {
        Self {
            vtable: StaticVTableRef(vtable.into()),
            type_info: PhantomData,
        }
    }
}

/// Specifies the VTable type that should be used and what auto traits should be
/// enforced for all type erased object.
///
/// The [`define_v_table`] macro will implement this for the trait object type
/// of the provided trait.
///
/// The `C` type parameter is the common data that is stored inside the type
/// erased allocation. This is necessary since the common data affects how the
/// allocation is freed so we need a different vtable for each common data type.
pub trait ThinTrait<C> {
    /// The type of the vtable.
    type VTable: auto_traits::VTableEnforcedAutoTraits
        + auto_traits::EnforceAutoTraits<
            <Self::VTable as auto_traits::VTableEnforcedAutoTraits>::UncheckedAutoTraitMarker,
        > + VTableDrop<C>;
    /// Indicates what auto traits should be enforced for the erased type.
    type AutoTraitConfig: auto_traits::AutoTraitConfig<
        <Self::VTable as auto_traits::VTableEnforcedAutoTraits>::UncheckedAutoTraitMarker,
    >;
}

/// The auto traits marker type for a `ThinTrait` implementor `V` for a certain
/// "common" data type `C`.
type ThinTraitAutoTraitsMarker<V, C> = auto_traits::AutoTraitConfigMarkerType<
    <V as ThinTrait<C>>::VTable,
    <V as ThinTrait<C>>::AutoTraitConfig,
>;

/// This trait should be implemented for a vtable to allow using it in a type's
/// [`Drop`] implementation.
pub trait VTableDrop<C>: Sized {
    /// Drop an erased box.
    ///
    /// # Safety
    ///
    /// - The type erased box must contain an object with the same type as the one
    /// that the vtable manages.
    unsafe fn drop_erased_box(
        &self,
        erased_box: RawThinBox<'_, Self, Taken<C>, auto_traits::NoAutoTraits, ()>,
    );
}

/// Gets a vtable with that has sensible behavior for the `T` type.
pub trait GetThinTraitVTable<T>: Sized {
    /// Get a vtable that has sensible behavior for the type `T`.
    fn get_vtable() -> VTable<Self, T>;
}

pub mod auto_traits {
    //! Specifies what auto traits a thin trait implements.

    use core::marker::PhantomData;

    /// Allows a vtable to guarantee that some auto traits will always be implemented.
    /// The vtable type must also implement [`EnforceAutoTraits`] on itself to ensure
    /// a [`VTable`](super::VTable) type can't be constructed for a type that doesn't
    /// implement the correct auto traits.
    pub trait VTableEnforcedAutoTraits {
        /// A type that implements all auto traits that are enforced. If its possible
        /// to construct a vtable for a type that doesn't implement all of these auto
        /// traits then that can lead to unsound behavior. Ensure that `GetThinTraitVTable<V>`
        /// is only implemented for types `V` that implements all of the auto traits.
        ///
        /// This is useful since a trait object implements all auto traits that are
        /// added as supertraits to its trait. So we can set this type to a trait
        /// object and then that trait will enforce the auto traits if we use it as
        /// a trait bound in the [`GetThinTraitVTable`](super::GetThinTraitVTable)
        /// implementation.
        type UncheckedAutoTraitMarker: ?Sized;
    }

    /// The auto traits marker type that implements the auto traits guaranteed by
    /// a vtable (`VTable`) with a certain auto trait config (`Config`).
    pub type AutoTraitConfigMarkerType<VTable, Config> = <Config as AutoTraitConfig<
        <VTable as VTableEnforcedAutoTraits>::UncheckedAutoTraitMarker,
    >>::MarkerType;

    /// Used to enforce that a type (`Self`) has the auto traits specified by an
    /// [`AutoTraitConfig`] (specified with the `A` type parameter).
    ///
    /// # Safety
    ///
    /// This trait must uphold the same safety invariants as the [`EnforceAutoTraits`]
    /// trait.
    pub unsafe trait HasAutoTraits<A: ?Sized> {}
    unsafe impl<T: ?Sized, A: ?Sized> HasAutoTraits<A> for T where A: EnforceAutoTraits<T> {}

    /// Implementation details for [`HasAutoTraits`]. This is implement on all
    /// "auto trait config" types for types `T` that have the correct auto traits.
    ///
    /// This trait is also implemented on vtable types for types `T` that uphold
    /// the vtable default auto traits specified via the vtable type's
    /// [`VTableEnforcedAutoTraits::UncheckedAutoTraitMarker`].
    ///
    /// # Safety
    ///
    /// If the `Self` type implements [`AutoTraitConfig`] then it must do so in
    /// a way that ensures the resulting marker type ([`AutoTraitConfig::MarkerType`])
    /// only implements the auto traits that are enforced/guaranteed by this trait.
    pub unsafe trait EnforceAutoTraits<T: ?Sized> {}

    /// Implemented for config types that enforce that certain auto traits are
    /// implemented for a type erased object.
    ///
    /// # Safety
    ///
    /// The specified `MarkerType` must not implement auto traits that are not
    /// enforced via the the [`EnforceAutoTraits`] trait.
    pub unsafe trait AutoTraitConfig<T: ?Sized> {
        /// A type that implements all auto traits of `T` and all auto traits
        /// that are enforced by the auto traits config.
        type MarkerType: ?Sized;
    }

    // Base case:
    unsafe impl<T: ?Sized> EnforceAutoTraits<T> for () {}
    unsafe impl<T: ?Sized> AutoTraitConfig<T> for () {
        type MarkerType = T;
    }

    /// Use this to ensure we never construct a wrapper type.
    #[derive(Debug)]
    enum Never {}

    macro_rules! define_auto_trait_wrappers {
        ($( $( #[$attr:meta] )* $wrapper_name:ident => $(#[unsafe] $(=>;;; never $is_unsafe:ident)?)? $auto_trait:path ),* $(,)?) => {
            $(
                $(#[$attr])*
                #[derive(Debug)]
                pub struct $wrapper_name<I: ?Sized>(PhantomData<I>, Never);
                unsafe impl<T: ?Sized, I: ?Sized> EnforceAutoTraits<T> for $wrapper_name<I>
                where
                    I: EnforceAutoTraits<T>,
                    T: $auto_trait,
                {
                }
                unsafe impl<T: ?Sized, I: ?Sized> AutoTraitConfig<T> for $wrapper_name<I> where I: AutoTraitConfig<T> {
                    type MarkerType = $wrapper_name<<I as AutoTraitConfig<T>>::MarkerType>;
                }
                define_auto_trait_wrappers!(@if ($(true $($is_unsafe)?)?) {
                    unsafe impl<I: ?Sized> $auto_trait for $wrapper_name<I> {}
                } else {
                    impl<I: ?Sized> $auto_trait for $wrapper_name<I> {}
                });
            )*
        };
        (@if (true $($condition:tt)*) { $($true:tt)* } else { $($false:tt)* } ) => { $($true)* };
        (@if ($($condition:tt)*) { $($true:tt)* } else { $($false:tt)* } ) => { $($false)* };
    }
    define_auto_trait_wrappers!(
        /// Ensures that an erased type implements the [`Send`] auto trait.
        HasSend => #[unsafe] Send,
        /// Ensures that an erased type implements the [`Sync`] auto trait.
        HasSync => #[unsafe] Sync,
        /// Ensures that an erased type implements the [`Unpin`] auto trait.
        HasUnpin => Unpin,
    );

    #[cfg(feature = "std")]
    define_auto_trait_wrappers!(
        /// Ensures that an erased type implements the [`std::panic::UnwindSafe`] auto trait.
        HasUnwindSafe => std::panic::UnwindSafe,
        /// Ensures that an erased type implements the [`std::panic::RefUnwindSafe`] auto trait.
        HasRefUnwindSafe => std::panic::RefUnwindSafe,
    );

    /// Trait objects for a trait without supertraits don't implement any auto traits.
    trait NoAutoTraitsTrait {}

    /// This type relaxes all auto trait requirements. The marker type indicates
    /// that no auto trait should be implemented for a type erased object.
    #[derive(Debug)]
    pub struct NoAutoTraits(PhantomData<dyn NoAutoTraitsTrait>, Never);
    unsafe impl<T: ?Sized> EnforceAutoTraits<T> for NoAutoTraits {}
    unsafe impl<T: ?Sized> AutoTraitConfig<T> for NoAutoTraits {
        type MarkerType = NoAutoTraits;
    }
}

/// The same as [`ThinBox`] except the common data has been moved out and is no
/// longer available.
#[repr(transparent)]
pub struct ThinBoxWithoutCommon<'erased_data, V, C>
where
    V: ThinTrait<C> + ?Sized,
{
    inner: ManuallyDrop<Box<ThinWithoutCommon<'erased_data, V, C>>>,
}
impl<'erased_data, V> ThinBoxWithoutCommon<'erased_data, V, ()>
where
    V: ThinTrait<()> + ?Sized,
{
    /// Create a new [`ThinBoxWithoutCommon`] that stores some data in a heap allocation.
    pub fn new<T>(x: T) -> Self
    where
        T: auto_traits::HasAutoTraits<V::AutoTraitConfig> + 'erased_data,
        V::VTable: GetThinTraitVTable<T>,
    {
        Self::from_raw(
            RawThinBox::new(x, ())
                .free_common_data()
                .with_auto_trait_config::<V::AutoTraitConfig>()
                .erase(),
        )
    }
}
impl<'erased_data, V, C> ThinBoxWithoutCommon<'erased_data, V, C>
where
    V: ThinTrait<C> + ?Sized,
{
    /// Put some common data into the heap allocation that stores the type erased
    /// object.
    pub fn put_common(this: Self, common: C) -> ThinBox<'erased_data, V, C> {
        ThinBox::from_raw(Self::into_raw(this).put_common_data(common))
    }

    /// Convert a [`ThinBoxWithoutCommon`] to a [`RawThinBox`]. This allows for
    /// a lower level, more powerful, API.
    pub fn into_raw(
        this: Self,
    ) -> RawThinBox<
        'erased_data,
        <V as ThinTrait<C>>::VTable,
        Taken<C>,
        ThinTraitAutoTraitsMarker<V, C>,
        (),
    > {
        let mut this = ManuallyDrop::new(this);
        // Safety: the `ManuallyDrop` wrapper ensures we never use `this` again.
        unsafe { Self::take_raw(&mut *this) }
    }

    /// Take a [`ThinBoxWithoutCommon`] and create a [`RawThinBox`] instead. This
    /// allows for a lower level, more powerful, API.
    ///
    /// # Safety
    ///
    /// `Self` must never be used after this function has been called, this includes ensuring that
    /// `Self` is not dropped.
    pub unsafe fn take_raw(
        this: &mut Self,
    ) -> RawThinBox<
        'erased_data,
        <V as ThinTrait<C>>::VTable,
        Taken<C>,
        ThinTraitAutoTraitsMarker<V, C>,
        (),
    > {
        // Safety: the read value won't ever be used from a safe function `RawThinBox`
        // doesn't make any guarantees about the state of its content, so the user
        // must assume that the wrapped value could already be dropped and can't do
        // anything to it safely.
        let inner: ManuallyDrop<Box<ThinWithoutCommon<'erased_data, V, C>>> =
            ptr::read(&this.inner);

        // Safety: `ThinWithoutCommon` is a `repr(transparent)` struct around `RawThin` and we ensured
        // the types inside `ThinWithoutCommon` lines up with the ones inside RawThin.
        let inner = mem::transmute::<
            ManuallyDrop<Box<ThinWithoutCommon<'erased_data, V, C>>>,
            ManuallyDrop<
                Box<
                    RawThin<
                        'erased_data,
                        <V as ThinTrait<C>>::VTable,
                        Taken<C>,
                        ThinTraitAutoTraitsMarker<V, C>,
                        (),
                    >,
                >,
            >,
        >(inner);
        RawThinBox { inner }
    }
    /// Convert a [`RawThinBox`] to a [`ThinBoxWithoutCommon`]. This allows for
    /// a more convent, more higher level API.
    pub fn from_raw(
        raw: RawThinBox<
            'erased_data,
            <V as ThinTrait<C>>::VTable,
            Taken<C>,
            ThinTraitAutoTraitsMarker<V, C>,
            (),
        >,
    ) -> Self {
        let inner = raw.inner;
        // Safety: `ThinWithoutCommon` is a `repr(transparent)` struct around `RawThin` and we ensured
        // the types inside `ThinWithoutCommon` lines up with the ones inside RawThin.
        let inner = unsafe {
            mem::transmute::<
                ManuallyDrop<
                    Box<
                        RawThin<
                            'erased_data,
                            <V as ThinTrait<C>>::VTable,
                            Taken<C>,
                            ThinTraitAutoTraitsMarker<V, C>,
                            (),
                        >,
                    >,
                >,
                ManuallyDrop<Box<ThinWithoutCommon<'erased_data, V, C>>>,
            >(inner)
        };
        Self { inner }
    }
}
impl<'erased_data, V, C> Deref for ThinBoxWithoutCommon<'erased_data, V, C>
where
    V: ThinTrait<C> + ?Sized,
{
    type Target = ThinWithoutCommon<'erased_data, V, C>;

    fn deref(&self) -> &Self::Target {
        &**self.inner
    }
}
impl<'erased_data, V, C> DerefMut for ThinBoxWithoutCommon<'erased_data, V, C>
where
    V: ThinTrait<C> + ?Sized,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut **self.inner
    }
}
impl<V, C> fmt::Debug for ThinBoxWithoutCommon<'_, V, C>
where
    V: ThinTrait<C> + ?Sized,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple(get_type_name!(ThinBoxWithoutCommon))
            .field(&**self.inner)
            .finish()
    }
}
impl<V, C> Drop for ThinBoxWithoutCommon<'_, V, C>
where
    V: ThinTrait<C> + ?Sized,
{
    fn drop(&mut self) {
        // Safety: Taking self is safe since it won't be accessed after this point.
        unsafe { Self::take_raw(self) }.free_via_vtable();
    }
}

/// A type erased object stored on the heap without using a fat pointer.
#[repr(transparent)]
pub struct ThinBox<'erased_data, V, C>
where
    V: ThinTrait<C> + ?Sized,
{
    inner: ManuallyDrop<Box<Thin<'erased_data, V, C>>>,
}
impl<'erased_data, V, C> ThinBox<'erased_data, V, C>
where
    V: ThinTrait<C> + ?Sized,
{
    /// Create a new [`ThinBox`] that stores some data in a heap allocation.
    pub fn new<T>(x: T, common: C) -> Self
    where
        T: auto_traits::HasAutoTraits<V::AutoTraitConfig> + 'erased_data,
        V::VTable: GetThinTraitVTable<T>,
    {
        Self::from_raw(
            RawThinBox::new(x, common)
                .with_auto_trait_config::<V::AutoTraitConfig>()
                .erase(),
        )
    }
}
impl<'erased_data, V, C> ThinBox<'erased_data, V, C>
where
    V: ThinTrait<C> + ?Sized,
{
    /// Take the common data that is stored for this object out of the heap
    /// allocation.
    pub fn take_common(this: Self) -> (ThinBoxWithoutCommon<'erased_data, V, C>, C)
    where
        V: ThinTrait<Taken<C>>,
    {
        let (this, common) = Self::into_raw(this).take_common_data();
        let this = ThinBoxWithoutCommon::from_raw(this);
        (this, common)
    }

    /// Convert a [`ThinBox`] to a [`RawThinBox`]. This allows for a lower level,
    /// more powerful, API.
    pub fn into_raw(
        this: Self,
    ) -> RawThinBox<'erased_data, <V as ThinTrait<C>>::VTable, C, ThinTraitAutoTraitsMarker<V, C>, ()>
    {
        let mut this = ManuallyDrop::new(this);
        // Safety: the `ManuallyDrop` wrapper ensures we never use `this` again.
        unsafe { Self::take_raw(&mut *this) }
    }
    /// Take a [`ThinBox`] and create a [`RawThinBox`] instead. This allows for
    /// a lower level, more powerful, API.
    ///
    /// # Safety
    ///
    /// `Self` must never be used after this function has been called, this includes ensuring that
    /// `Self` is not dropped.
    pub unsafe fn take_raw(
        this: &mut Self,
    ) -> RawThinBox<'erased_data, <V as ThinTrait<C>>::VTable, C, ThinTraitAutoTraitsMarker<V, C>, ()>
    {
        // Safety: the read value won't ever be used from a safe function `RawThinBox`
        // doesn't make any guarantees about the state of its content, so the user
        // must assume that the wrapped value could already be dropped and can't do
        // anything to it safely.
        let inner: ManuallyDrop<Box<Thin<'erased_data, V, C>>> = ptr::read(&this.inner);

        // Safety: `Thin` is a `repr(transparent)` struct around `RawThin` and we ensured
        // the types inside `Thin` lines up with the ones inside RawThin.
        let inner = mem::transmute::<
            ManuallyDrop<Box<Thin<'erased_data, V, C>>>,
            ManuallyDrop<
                Box<
                    RawThin<
                        'erased_data,
                        <V as ThinTrait<C>>::VTable,
                        C,
                        ThinTraitAutoTraitsMarker<V, C>,
                        (),
                    >,
                >,
            >,
        >(inner);
        RawThinBox { inner }
    }
    /// Convert a [`RawThinBox`] to a [`ThinBox`]. This allows for a more convent,
    /// more higher level API.
    pub fn from_raw(
        raw: RawThinBox<
            'erased_data,
            <V as ThinTrait<C>>::VTable,
            C,
            ThinTraitAutoTraitsMarker<V, C>,
            (),
        >,
    ) -> Self {
        let inner = raw.inner;
        // Safety: `Thin` is a `repr(transparent)` struct around `RawThin` and we ensured
        // the types inside `Thin` lines up with the ones inside RawThin.
        let inner = unsafe {
            mem::transmute::<
                ManuallyDrop<
                    Box<
                        RawThin<
                            'erased_data,
                            <V as ThinTrait<C>>::VTable,
                            C,
                            ThinTraitAutoTraitsMarker<V, C>,
                            (),
                        >,
                    >,
                >,
                ManuallyDrop<Box<Thin<'erased_data, V, C>>>,
            >(inner)
        };
        Self { inner }
    }
}
impl<'erased_data, V, C> Deref for ThinBox<'erased_data, V, C>
where
    V: ThinTrait<C> + ?Sized,
{
    type Target = Thin<'erased_data, V, C>;

    fn deref(&self) -> &Self::Target {
        &**self.inner
    }
}
impl<'erased_data, V, C> DerefMut for ThinBox<'erased_data, V, C>
where
    V: ThinTrait<C> + ?Sized,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut **self.inner
    }
}
impl<V, C> fmt::Debug for ThinBox<'_, V, C>
where
    V: ThinTrait<C> + ?Sized,
    C: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple(get_type_name!(ThinBox))
            .field(&**self.inner)
            .finish()
    }
}
impl<V, C> Drop for ThinBox<'_, V, C>
where
    V: ThinTrait<C> + ?Sized,
{
    fn drop(&mut self) {
        // Safety: Taking self is safe since it won't be accessed after this point.
        unsafe { Self::take_raw(self) }
            .free_common_data()
            .free_via_vtable();
    }
}

/// A lower level API for [`ThinBox`]. Note that if this is dropped then the
/// underlying memory won't be freed.
#[repr(transparent)]
pub struct RawThinBox<'erased_data, V, C, M, D>
where
    M: ?Sized,
{
    inner: ManuallyDrop<Box<RawThin<'erased_data, V, C, M, D>>>,
}
impl<'erased_data, V, C, D>
    RawThinBox<'erased_data, V, C, auto_traits::AutoTraitConfigMarkerType<V, ()>, Unerased<D>>
where
    V: auto_traits::VTableEnforcedAutoTraits,
    V: GetThinTraitVTable<D>,
    D: 'erased_data,
{
    /// Create a new [`RawThinBox`] that stores some data in a heap allocation.
    ///
    /// Note that [`RawThinBox`] is quite a low level API so prefer [`ThinBox::new`]
    /// or [`ThinBoxWithoutCommon::new`].
    pub fn new(x: D, common: C) -> Self {
        Self {
            inner: ManuallyDrop::new(Box::new(RawThin {
                vtable: V::get_vtable().vtable,
                common,
                // Safety: we use the default auto trait config marker for this
                // vtable which should always be safe.
                _not_send_or_sync: PhantomData,
                _lifetime: PhantomData,
                _object: Unerased::new(x),
            })),
        }
    }
}
impl<V, C, M, D> fmt::Debug for RawThinBox<'_, V, C, M, D>
where
    M: ?Sized,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(get_type_name!(RawThinBox)).finish()
    }
}
impl<'erased_data, V, C, M, D> RawThinBox<'erased_data, V, C, M, D>
where
    M: ?Sized,
{
    /// Erase the type of the stored object.
    pub fn erase(self) -> RawThinBox<'erased_data, V, C, M, ()> {
        // Static assertions about the `()` type for clarity:
        const _: [(); 0] = [(); mem::size_of::<()>()];
        const _: [(); 1] = [(); mem::align_of::<()>()];

        // Safety: `()` has 0 size and the weakest alignment (`1`) so it shouldn't
        // cause any issues.
        unsafe { mem::transmute(self) }
    }
    /// Forget the lifetime of the type erased object.
    ///
    /// # Safety
    ///
    /// The returned type can be kept around longer than the erased data will
    /// be alive and allow for use-after-free bugs (via dangling pointers).
    pub unsafe fn forget_lifetime(self) -> RawThinBox<'static, V, C, M, D> {
        // Safety:
        // we are only transmuting a lifetime so the layout of the type will be
        // the same.
        mem::transmute(self)
    }
    /// Weaken the auto traits marker type to the weakest it can be. The returned
    /// type won't implement any auto traits even if it would be safe to do so.
    pub fn weaken_auto_traits_marker(
        self,
    ) -> RawThinBox<'erased_data, V, C, auto_traits::NoAutoTraits, D> {
        // Safety:
        // The marker type is stored inside a `PhantomData` type so it never
        // affects the layout of the `RawThin` type.
        //
        // The returned type might incorrect implement auto traits that aren't actually
        // guaranteed by the erased type. This could for example allow sending a type
        // to another thread even though the type doesn't implement `Send`.
        // This isn't an issue since `auto_traits::NoAutoTraits` ensures the type
        // doesn't implement any auto traits even if it would be safe to do so.
        unsafe { mem::transmute(self) }
    }
    /// Drop the common data in place.
    pub fn free_common_data(self) -> RawThinBox<'erased_data, V, Taken<C>, M, D> {
        // Safety: `Taken<C>` is a `repr(transparent)` wrapper around `C`
        let mut taken: RawThinBox<'erased_data, V, Taken<C>, M, D> =
            unsafe { mem::transmute(self) };
        let wrapper: &mut Taken<_> = &mut taken.inner.common;
        // Safety: we just owned `C` so it is safe to drop it, `Taken` will ensure
        // we never touch it again. If `C` is already a `Taken` struct then it doesn't
        // do anything when dropped and this would be a noop.
        unsafe { ManuallyDrop::drop(&mut wrapper.0) };
        taken
    }
    /// Take the common data from the allocation.
    pub fn take_common_data(self) -> (RawThinBox<'erased_data, V, Taken<C>, M, D>, C) {
        // Safety: `Taken<C>` is a `repr(transparent)` wrapper around `C`
        let mut taken: RawThinBox<'erased_data, V, Taken<C>, M, D> =
            unsafe { mem::transmute(self) };
        let wrapper: &mut Taken<_> = &mut taken.inner.common;
        // Safety: we just owned `C` so it is safe to take it, `Taken` will ensure
        // we never touch it again.
        let common = unsafe { ManuallyDrop::take(&mut wrapper.0) };
        (taken, common)
    }
}
/// Methods that are only available after the common data has been taken or freed.
impl<'erased_data, V, C, M, D> RawThinBox<'erased_data, V, Taken<C>, M, D>
where
    M: ?Sized,
{
    /// Put some common data into the allocation.
    pub fn put_common_data(mut self, common: C) -> RawThinBox<'erased_data, V, C, M, D> {
        // Safety: we write some valid common data to the allocation and then
        // we remove the `repr(transparent)` wrapper `Taken` to indicate that
        // the common data is in a state where it can be used.
        unsafe {
            ptr::write(&mut self.inner.common.0, ManuallyDrop::new(common));
            mem::transmute(self)
        }
    }
    /// Use the vtable to free the wrapped box.
    ///
    /// The common data must have been freed before the vtable is used to free the
    /// allocation. This is to support taking tha common data out of the allocation.
    /// You can free the common data using the [`free_common_data`](Self::free_common_data)
    /// method.
    pub fn free_via_vtable(self)
    where
        V: VTableDrop<C>,
    {
        // Get the vtable before we free the allocation:
        let vtable = self.inner.vtable.static_ref();

        // Safety: the vtable manages a type that is the same as the one in `self`.
        unsafe {
            V::drop_erased_box(
                vtable,
                self
                    // We forget the auto traits marker since it is only a zero sized `PhantomData`
                    // and shouldn't affect the type's layout. (We don't want to generate one vtable per
                    // auto traits marker type.)
                    .weaken_auto_traits_marker()
                    // The object type is probably already erased, but this call
                    // allows this method to be called even when it isn't.
                    .erase(),
            )
        };
    }
}
impl<'erased_data, V, C, M> RawThinBox<'erased_data, V, C, M, ()>
where
    M: ?Sized,
{
    /// Unerase the erased type.
    ///
    /// # Safety
    ///
    /// The type specified via the `D2` type parameter must be the actual type of
    /// the type erased object that is stored inside this allocation.
    pub unsafe fn unerase<D2>(self) -> RawThinBox<'erased_data, V, C, M, Unerased<D2>> {
        mem::transmute(self)
    }
}
/// These methods require that the object's type is known.
impl<'erased_data, V, C, M, D> RawThinBox<'erased_data, V, Taken<C>, M, Unerased<D>>
where
    M: ?Sized,
{
    /// Take the wrapped object out and then free the allocation.
    pub fn into_inner(self) -> D {
        // Safety: `Taken` is `repr(transparent)`
        let mut this = unsafe {
            mem::transmute::<
                RawThinBox<'erased_data, V, Taken<C>, M, Unerased<D>>,
                RawThinBox<'erased_data, V, Taken<C>, M, Unerased<Taken<D>>>,
            >(self)
        };
        // Safety: the `Taken` wrapper ensures that the object won't be touched again,
        // even when the box is freed.
        let object = unsafe { ManuallyDrop::take(&mut (this.inner._object.0).0) };
        this.free();
        object
    }
}
/// These methods require that the object's type is known.
impl<'erased_data, V, C, M, D> RawThinBox<'erased_data, V, C, M, Unerased<D>>
where
    M: ?Sized,
{
    /// Change the auto traits config that determines what auto traits are
    /// implemented.
    pub fn with_auto_trait_config<A>(
        self,
    ) -> RawThinBox<'erased_data, V, C, auto_traits::AutoTraitConfigMarkerType<V, A>, Unerased<D>>
    where
        // Allows us to get a type that implements some default auto traits that
        // are required by the vtable (supertraits of the vtable trait):
        V: auto_traits::VTableEnforcedAutoTraits,
        // Ensure we can get the marker type for the auto trait config:
        A: auto_traits::AutoTraitConfig<V::UncheckedAutoTraitMarker>,
        // Enforces any extra auto traits from the auto trait config for the
        // stored type:
        D: auto_traits::HasAutoTraits<A>,
    {
        // Safety:
        // The marker type that we are changing with this transmute is wrapped
        // inside a `PhantomData` and so won't affect the type's layout.
        //
        // the `D: auto_traits::HasAutoTraits<A>` trait bound ensures that
        // the the stored type implements the auto traits that the marker type
        // requires.
        unsafe { mem::transmute(self) }
    }
    /// Free the wrapped box.
    pub fn free(mut self) {
        // Safety: the wrapped type must have been unerased before calling this
        // for example using the `unerase` method, otherwise the `D` type couldn't
        // be wrapped in `Unerased`.
        unsafe { ManuallyDrop::drop(&mut self.inner) }
    }
}

/// Guarantees that the wrapped value isn't type erased.
#[repr(transparent)]
#[derive(Debug)]
pub struct Unerased<T>(T);
impl<T> Unerased<T> {
    fn new(value: T) -> Self {
        Self(value)
    }
    /// Take the value out the wrapper.
    pub fn into_inner(self) -> T {
        self.0
    }
}
impl<T> Deref for Unerased<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl<T> DerefMut for Unerased<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

/// Indicates that the wrapped common data has been taken out of an allocation
/// and can't be used anymore.
#[repr(transparent)]
pub struct Taken<T>(ManuallyDrop<T>);
impl<T> fmt::Debug for Taken<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple(get_type_name!(Taken)).finish()
    }
}

/// Indicates that the wrapped value is being used from another reference.
pub struct Split<T>(PhantomData<T>);
impl<T> fmt::Debug for Split<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple(get_type_name!(Split)).finish()
    }
}

/// Use this to get a thin pointer to a type that is stored on the stack.
pub struct OwnedThin<V, C, T>
where
    V: ThinTrait<C> + ?Sized,
{
    inner: RawThin<'static, <V as ThinTrait<C>>::VTable, C, ThinTraitAutoTraitsMarker<V, C>, T>,
}
impl<V, C, T> OwnedThin<V, C, T>
where
    V: ThinTrait<C> + ?Sized,
{
    /// Create a wrapper around some data that allows that data to be borrowed as
    /// a thin pointer even though the type of the stored data is erased.
    pub fn new(x: T, common: C) -> Self
    where
        T: auto_traits::HasAutoTraits<V::AutoTraitConfig>,
        V::VTable: GetThinTraitVTable<T>,
    {
        Self {
            inner: RawThin {
                vtable: V::VTable::get_vtable().vtable,
                common,
                _not_send_or_sync: PhantomData,
                _lifetime: PhantomData,
                _object: x,
            },
        }
    }
    /// Take the stored data out of the thin trait wrapper.
    pub fn into_inner(self) -> (T, C) {
        (self.inner._object, self.inner.common)
    }
}
impl<V, C, T> Deref for OwnedThin<V, C, T>
where
    V: ThinTrait<C> + ?Sized,
{
    type Target = Thin<'static, V, C>;

    fn deref(&self) -> &Self::Target {
        // Safety: `Thin` is a transparent wrapper around `Inner` where the type that
        // is stored is erased.
        let thin = &self.inner as *const _ as *const Thin<'static, V, C>;
        unsafe { &*thin }
    }
}
impl<V, C, T> DerefMut for OwnedThin<V, C, T>
where
    V: ThinTrait<C> + ?Sized,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        // Safety: `Thin` is a transparent wrapper around `Inner` where the type that
        // is stored is erased.
        let thin = &mut self.inner as *mut _ as *mut Thin<'static, V, C>;
        unsafe { &mut *thin }
    }
}
impl<V, C, T> fmt::Debug for OwnedThin<V, C, T>
where
    V: ThinTrait<C> + ?Sized,
    C: fmt::Debug,
    T: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(get_type_name!(OwnedThin))
            .field("common", &self.inner.common)
            .field("object", &self.inner._object)
            .finish()
    }
}

/// A value whose type has been erased. This can only be used through a reference.
#[repr(transparent)]
pub struct Thin<'erased_data, V, C>
where
    V: ThinTrait<C> + ?Sized,
{
    inner:
        RawThin<'erased_data, <V as ThinTrait<C>>::VTable, C, ThinTraitAutoTraitsMarker<V, C>, ()>,
}
impl<'erased_data, V, C> Thin<'erased_data, V, C>
where
    V: ThinTrait<C> + ?Sized,
{
    /// Borrow the common data and the type erased object at the same time.
    pub fn split_common(this: &Self) -> (&Thin<'erased_data, V, C>, &C) {
        let common = &this.inner.common;
        (this, common)
    }
    /// Borrow the common data and the type erased object at the same time.
    pub fn split_common_mut(
        this: &mut Self,
    ) -> (&mut ThinWithoutCommon<'erased_data, V, C>, &mut C) {
        // This is a bit tricky to make this safe since the `&mut C` type would
        // be pointing inside of the `&mut RawThin<VTable, C, Marker, ()>` type. Since
        // `&mut` references are unique this would be a bit strange to the compiler.
        // We basically want to support a type layout that has a hole inside it.
        //
        // The solution is to use some kind of offset_of pointer calculations to go directly from a
        // reference/pointer to the `&VTable` type at `RawThin::vtable` to the erased data at `RawThin::_object`
        // without construction a reference to `RawThin` and implicitly telling the compiler that we also own the
        // common data at `RawThin::common`. Since the `RawThin` type is `repr(C)` we can actually do all of this
        // without compiler support like RFC 2582 (&raw) (https://github.com/Gilnaa/memoffset#raw-references).

        // Safety: the `ThinWithoutCommon` reference can't be used to access to common
        // data so another `&mut C` reference can't be created by safe code. The
        // `ThinWithoutCommon` type also has its `C` type wrapped inside `PhantomData`
        // via the `Split` type so the layout of data behind the two reference won't
        // overlap.

        let this: *mut Self = this;
        let thin: *mut ThinWithoutCommon<'erased_data, V, C> = this as _;
        let common: *mut C = RawThin::offset_to_common(
            this as *mut RawThin<
                'erased_data,
                <V as ThinTrait<C>>::VTable,
                Split<C>,
                ThinTraitAutoTraitsMarker<V, C>,
                (),
            >,
        );
        unsafe { (&mut *thin, &mut *common) }
    }
}
impl<'erased_data, V, C> Deref for Thin<'erased_data, V, C>
where
    V: ThinTrait<C> + ?Sized,
{
    type Target = ThinWithoutCommon<'erased_data, V, C>;
    fn deref(&self) -> &Self::Target {
        // Safety: `Thin` and `ThinWithoutCommon` are both `#[repr(transparent)]`
        // wrappers around the `RawThin` type. The only difference is that
        // `ThinWithoutCommon` has the common type wrapped in `Taken`. `Taken`
        // only weakens requirements so it should be safe to access the data in
        // that way.
        unsafe { &*((self as *const Self) as *const ThinWithoutCommon<'erased_data, V, C>) }
    }
}
impl<'erased_data, V, C> DerefMut for Thin<'erased_data, V, C>
where
    V: ThinTrait<C> + ?Sized,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        // Safety: `Thin` and `ThinWithoutCommon` are both `#[repr(transparent)]`
        // wrappers around the `RawThin` type. The only difference is that
        // `ThinWithoutCommon` has the common type wrapped in `Taken`. `Taken`
        // only weakens requirements so it should be safe to access the data in
        // that way.
        unsafe { &mut *((self as *mut Self) as *mut ThinWithoutCommon<'erased_data, V, C>) }
    }
}
impl<V, C> fmt::Debug for Thin<'_, V, C>
where
    V: ThinTrait<C> + ?Sized,
    C: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (_, common) = Self::split_common(self);
        f.debug_struct(get_type_name!(Thin))
            .field("common", &common)
            .finish()
    }
}

/// A value whose type has been erased. This can only be used through a reference.
/// The common data stored with the type erased object has been taken (moved out
/// of the allocation) or is being accessed through another reference and can not
/// be accessed through this type.
#[repr(transparent)]
pub struct ThinWithoutCommon<'erased_data, V, C>
where
    V: ThinTrait<C> + ?Sized,
{
    inner: RawThin<
        'erased_data,
        <V as ThinTrait<C>>::VTable,
        Split<C>,
        ThinTraitAutoTraitsMarker<V, C>,
        (),
    >,
}
impl<'erased_data, V, C> ThinWithoutCommon<'erased_data, V, C>
where
    V: ThinTrait<C> + ?Sized,
{
    /// Get access to the vtable.
    ///
    /// # Safety
    ///
    /// The methods in the vtable must only be called in sensible ways.
    pub unsafe fn get_vtable<'a>(this: &Self) -> &'a <V as ThinTrait<C>>::VTable {
        this.inner.vtable.static_ref()
    }
    /// Get a more low level API for the type erased reference.
    pub fn as_raw(
        this: &Self,
    ) -> &RawThin<
        'erased_data,
        <V as ThinTrait<C>>::VTable,
        Split<C>,
        ThinTraitAutoTraitsMarker<V, C>,
        (),
    > {
        &this.inner
    }
    /// Get a more low level API for the type erased reference.
    pub fn as_raw_mut(
        this: &mut Self,
    ) -> &mut RawThin<
        'erased_data,
        <V as ThinTrait<C>>::VTable,
        Split<C>,
        ThinTraitAutoTraitsMarker<V, C>,
        (),
    > {
        &mut this.inner
    }
}
impl<V, C> fmt::Debug for ThinWithoutCommon<'_, V, C>
where
    V: ThinTrait<C> + ?Sized,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(get_type_name!(ThinWithoutCommon)).finish()
    }
}

/// Stores an erased type and a vtable for interacting with it.
///
/// - `V` is the type of the vtable.
/// - `C` is the type of some common data that is shared between all erased types.
///   This allows for accessing common data without a virtual method call.
/// - `M` a marker type placed inside `PhantomData` that ensures this type only
///   implements the auto traits that are safe. Use `dyn DummyTrait` to opt out
///   of all auto traits or `*mut ()` to opt out of `Send` and `Sync`.
/// - `L` a marker type that ensures the lifetime of the type erased object is
///   respected.
/// - `D` is the type that is erased and is later only accessible via the vtable.
///
/// `repr(C)` to ensure that D remains in the final position.
#[repr(C)]
pub struct RawThin<'erased_data, V, C, M, D>
where
    M: ?Sized,
{
    /// A vtable that provides methods that can access the type erased `_object`.
    vtable: StaticVTableRef<V>,
    /// Common data shared between all erased types.
    common: C,
    /// When we erase the type of `D` we can't know if it was `Send` or `Sync`.
    /// If we ensure that the type is only constructed from types that do implement
    /// Send and/or Sync then we could remove this restriction (for example using
    /// a wrapper type that unsafely implements Send or by specifying this type
    /// via an associated type).
    _not_send_or_sync: PhantomData<M>,
    /// A marker type that ensures the lifetime of the type erased object is
    /// respected.
    _lifetime: PhantomData<&'erased_data ()>,
    /// NOTE: Don't use directly. Use only through vtable. Erased type may have
    /// different alignment.
    _object: D,
}
impl<'erased_data, V, C, M> RawThin<'erased_data, V, C, M, ()>
where
    M: ?Sized,
{
    /// Unerase the erased type.
    ///
    /// # Safety
    ///
    /// The type specified via the `D2` type parameter must be the actual type of
    /// the type erased object that is stored inside this allocation.
    pub unsafe fn as_unerase<D2>(&self) -> &RawThin<'erased_data, V, C, M, Split<D2>> {
        &*((self as *const Self) as *const RawThin<'erased_data, V, C, M, Split<D2>>)
    }
    /// Unerase the object.
    ///
    /// # Safety
    ///
    /// The type specified via the `D2` type parameter must be the actual type of
    /// the type erased object that is stored inside this allocation.
    pub unsafe fn as_unerase_mut<D2>(&mut self) -> &mut RawThin<'erased_data, V, C, M, Split<D2>> {
        &mut *((self as *mut Self) as *mut RawThin<'erased_data, V, C, M, Split<D2>>)
    }
}
impl<'erased_data, V, C, M, D> RawThin<'erased_data, V, Split<C>, M, D>
where
    M: ?Sized,
{
    /// Use pointer arithmetic to convert a pointer to a type erased allocation
    /// to a pointer to the common data stored inside it.
    pub fn offset_to_common(this: *mut Self) -> *mut C {
        let after_vtable = (this as *mut StaticVTableRef<V>).wrapping_add(1) as *mut u8;
        let offset_to_common = after_vtable.align_offset(mem::align_of::<C>());
        after_vtable.wrapping_add(offset_to_common) as *mut C
    }
}
impl<'erased_data, V, C, M, D> RawThin<'erased_data, V, Split<C>, M, Split<D>>
where
    M: ?Sized,
{
    /// Use pointer arithmetic to convert a pointer to a type erased allocation
    /// to a pointer to the object inside it.
    pub fn offset_to_object(this: *mut Self) -> *mut D {
        let after_common = Self::offset_to_common(this).wrapping_add(1) as *mut u8;
        let offset_to_object = after_common.align_offset(mem::align_of::<D>());
        after_common.wrapping_add(offset_to_object) as *mut D
    }
    /// Get access to the object that is normally type erased.
    pub fn as_object(&self) -> &D {
        unsafe { &*Self::offset_to_object((self as *const Self) as *mut Self) }
    }
    /// Get access to the object that is normally type erased.
    pub fn as_object_mut(&mut self) -> &mut D {
        unsafe { &mut *Self::offset_to_object((self as *const Self) as *mut Self) }
    }
}
impl<'erased_data, V, C, M, D> RawThin<'erased_data, V, C, M, D>
where
    M: ?Sized,
{
    /// Remove the lifetime marker.
    pub fn without_lifetime_marker(&self) -> &RawThin<'static, V, C, M, D> {
        // Safety:
        // The marker type is stored inside a `PhantomData` type so it never
        // affects the layout of the `RawThin` type.
        //
        // The returned type might live longer than the erased object. But the
        // self reference must live shorter than the erased data so it should
        // be fine.
        unsafe { &*mem::transmute::<*const Self, *const RawThin<'static, V, C, M, D>>(self as _) }
    }
    /// Remove the lifetime marker.
    pub fn without_lifetime_marker_mut(&mut self) -> &mut RawThin<'static, V, C, M, D> {
        // Safety:
        // The marker type is stored inside a `PhantomData` type so it never
        // affects the layout of the `RawThin` type.
        //
        // The returned type might live longer than the erased object. But the
        // self reference must live shorter than the erased data so it should
        // be fine.
        unsafe { &mut *mem::transmute::<*mut Self, *mut RawThin<'static, V, C, M, D>>(self as _) }
    }
    /// Weaken the auto traits marker type to the weakest it can be. The returned
    /// type won't implement any auto traits even if it would be safe to do so.
    pub fn as_weaker_auto_traits_marker(
        &self,
    ) -> &RawThin<'erased_data, V, C, auto_traits::NoAutoTraits, D> {
        // Safety:
        // The marker type is stored inside a `PhantomData` type so it never
        // affects the layout of the `RawThin` type.
        //
        // The returned type might incorrect implement auto traits that aren't actually
        // guaranteed by the erased type. This could for example allow sending a type
        // to another thread even though the type doesn't implement `Send`.
        // This isn't an issue since `auto_traits::NoAutoTraits` ensures the type
        // doesn't implement any auto traits even if it would be safe to do so.
        unsafe {
            &*((self as *const Self)
                as *const RawThin<'erased_data, V, C, auto_traits::NoAutoTraits, D>)
        }
    }
    /// Weaken the auto traits marker type to the weakest it can be. The returned
    /// type won't implement any auto traits even if it would be safe to do so.
    pub fn as_weaker_auto_traits_marker_mut(
        &mut self,
    ) -> &mut RawThin<'erased_data, V, C, auto_traits::NoAutoTraits, D> {
        // Safety:
        // The marker type is stored inside a `PhantomData` type so it never
        // affects the layout of the `RawThin` type.
        //
        // The returned type might incorrect implement auto traits that aren't actually
        // guaranteed by the erased type. This could for example allow sending a type
        // to another thread even though the type doesn't implement `Send`.
        // This isn't an issue since `auto_traits::NoAutoTraits` ensures the type
        // doesn't implement any auto traits even if it would be safe to do so.
        unsafe {
            &mut *((self as *mut Self)
                as *mut RawThin<'erased_data, V, C, auto_traits::NoAutoTraits, D>)
        }
    }
}
impl<'erased_data, V, C, M, D> fmt::Debug for RawThin<'erased_data, V, C, M, D>
where
    M: ?Sized,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(get_type_name!(RawThin)).finish()
    }
}

/// This should be equivalent to `&'static V`. The reason for this struct is that
/// the type system doesn't like `'static` references to types that contain lifetimes.
/// This might happen if the vtable is for a trait that has lifetimes.
struct StaticVTableRef<V>(NonNull<V>);
impl<V> StaticVTableRef<V> {
    fn static_ref<'a>(&self) -> &'a V {
        // Safety: the vtable pointer is created from a `'static` reference.
        // So creating a `'static` reference again should be safe (or any other
        // shorter lifetime).
        unsafe { &*self.0.as_ptr() }
    }
}
unsafe impl<V> Send for StaticVTableRef<V> where V: Send {}
unsafe impl<V> Sync for StaticVTableRef<V> where V: Sync {}
