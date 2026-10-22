// SPDX-License-Identifier: (MIT OR Apache-2.0)

use crate::{Error, Result};
use rustix::io::Errno;

/*
 * Drivers are configured via properties through the blkio_get_X()/blkio_set_X() APIs where X is a
 * data type.
 */

pub fn error_read_only() -> Error {
    Error::new(Errno::ACCESS, "Property is read-only")
}

pub fn error_must_be_connected() -> Error {
    Error::new(Errno::NODEV, "Device must be connected")
}

pub fn error_cant_set_while_connected() -> Error {
    Error::new(Errno::BUSY, "Cannot set property in connected state")
}

pub fn error_cant_set_while_started() -> Error {
    Error::new(Errno::BUSY, "Cannot set property in started state")
}

enum PropertyAccessor<T> {
    Bool(fn(&T) -> Result<bool>, fn(&mut T, bool) -> Result<()>),
    I32(fn(&T) -> Result<i32>, fn(&mut T, i32) -> Result<()>),
    Str(fn(&T) -> Result<String>, fn(&mut T, &str) -> Result<()>),
    U64(fn(&T) -> Result<u64>, fn(&mut T, u64) -> Result<()>),
}

pub struct Property<T> {
    name: String,
    accessor: PropertyAccessor<T>,
}

impl<T> Property<T> {
    fn make_property_name(name: &str) -> String {
        name.replace('_', "-")
    }

    pub fn new_bool(
        name: &str,
        get: fn(&T) -> Result<bool>,
        set: fn(&mut T, bool) -> Result<()>,
    ) -> Self {
        Property {
            name: Self::make_property_name(name),
            accessor: PropertyAccessor::Bool(get, set),
        }
    }

    pub fn new_i32(
        name: &str,
        get: fn(&T) -> Result<i32>,
        set: fn(&mut T, i32) -> Result<()>,
    ) -> Self {
        Property {
            name: Self::make_property_name(name),
            accessor: PropertyAccessor::I32(get, set),
        }
    }

    pub fn new_str(
        name: &str,
        get: fn(&T) -> Result<String>,
        set: fn(&mut T, &str) -> Result<()>,
    ) -> Self {
        Property {
            name: Self::make_property_name(name),
            accessor: PropertyAccessor::Str(get, set),
        }
    }

    pub fn new_u64(
        name: &str,
        get: fn(&T) -> Result<u64>,
        set: fn(&mut T, u64) -> Result<()>,
    ) -> Self {
        Property {
            name: Self::make_property_name(name),
            accessor: PropertyAccessor::U64(get, set),
        }
    }

    fn find<'a>(props: &'a [Property<T>], name: &str) -> Result<&'a PropertyAccessor<T>> {
        for prop in props {
            if name == prop.name {
                return Ok(&prop.accessor);
            }
        }
        Err(Error::new(Errno::NOENT, "Unknown property name"))
    }

    pub fn get_bool(props: &[Property<T>], name: &str, t: &T) -> Result<bool> {
        if let PropertyAccessor::Bool(get, _) = Property::find(props, name)? {
            get(t)
        } else {
            Err(Error::new(Errno::NOTTY, "Property is not a bool"))
        }
    }

    pub fn get_i32(props: &[Property<T>], name: &str, t: &T) -> Result<i32> {
        if let PropertyAccessor::I32(get, _) = Property::find(props, name)? {
            get(t)
        } else {
            Err(Error::new(Errno::NOTTY, "Property is not an int"))
        }
    }

    pub fn get_str(props: &[Property<T>], name: &str, t: &T) -> Result<String> {
        match Property::find(props, name)? {
            PropertyAccessor::Bool(get, _) => Ok(get(t)?.to_string()),
            PropertyAccessor::I32(get, _) => Ok(get(t)?.to_string()),
            PropertyAccessor::Str(get, _) => get(t),
            PropertyAccessor::U64(get, _) => Ok(get(t)?.to_string()),
        }
    }

    pub fn get_u64(props: &[Property<T>], name: &str, t: &T) -> Result<u64> {
        if let PropertyAccessor::U64(get, _) = Property::find(props, name)? {
            get(t)
        } else {
            Err(Error::new(
                Errno::NOTTY,
                "Property is not an unsigned 64-bit integer",
            ))
        }
    }

    pub fn set_bool(props: &[Property<T>], name: &str, t: &mut T, value: bool) -> Result<()> {
        if let PropertyAccessor::Bool(_, set) = Property::find(props, name)? {
            set(t, value)
        } else {
            Err(Error::new(Errno::NOTTY, "Property is not a bool"))
        }
    }

    pub fn set_i32(props: &[Property<T>], name: &str, t: &mut T, value: i32) -> Result<()> {
        if let PropertyAccessor::I32(_, set) = Property::find(props, name)? {
            set(t, value)
        } else {
            Err(Error::new(Errno::NOTTY, "Property is not an int"))
        }
    }

    pub fn set_str(props: &[Property<T>], name: &str, t: &mut T, value: &str) -> Result<()> {
        match Property::find(props, name)? {
            PropertyAccessor::Bool(_, set) => {
                if let Ok(bool_value) = value.parse::<bool>() {
                    set(t, bool_value)
                } else {
                    Err(Error::new(
                        Errno::INVAL,
                        "Value must be \"true\" or \"false\"",
                    ))
                }
            }
            PropertyAccessor::I32(_, set) => {
                if let Ok(i32_value) = value.parse::<i32>() {
                    set(t, i32_value)
                } else {
                    Err(Error::new(
                        Errno::INVAL,
                        "Value must be a signed 32-bit integer",
                    ))
                }
            }
            PropertyAccessor::Str(_, set) => set(t, value),
            PropertyAccessor::U64(_, set) => {
                if let Ok(u64_value) = value.parse::<u64>() {
                    set(t, u64_value)
                } else {
                    Err(Error::new(
                        Errno::INVAL,
                        "Value must be an unsigned 64-bit integer",
                    ))
                }
            }
        }
    }

    pub fn set_u64(props: &[Property<T>], name: &str, t: &mut T, value: u64) -> Result<()> {
        if let PropertyAccessor::U64(_, set) = Property::find(props, name)? {
            set(t, value)
        } else {
            Err(Error::new(
                Errno::NOTTY,
                "Property is not an unsigned 64-bit int",
            ))
        }
    }
}

pub trait Properties {
    fn get_bool(&self, name: &str) -> Result<bool>;
    fn get_i32(&self, name: &str) -> Result<i32>;
    fn get_str(&self, name: &str) -> Result<String>;
    fn get_u64(&self, name: &str) -> Result<u64>;
    fn set_bool(&mut self, name: &str, value: bool) -> Result<()>;
    fn set_i32(&mut self, name: &str, value: i32) -> Result<()>;
    fn set_str(&mut self, name: &str, value: &str) -> Result<()>;
    fn set_u64(&mut self, name: &str, value: u64) -> Result<()>;
}

pub trait PropertiesList: Properties + Sized {
    fn get_props<'a>() -> &'a Vec<Property<Self>>;
}

impl<T: PropertiesList> Properties for T {
    fn get_bool(&self, name: &str) -> Result<bool> {
        Property::get_bool(Self::get_props(), name, self)
    }

    fn get_i32(&self, name: &str) -> Result<i32> {
        Property::get_i32(Self::get_props(), name, self)
    }

    fn get_str(&self, name: &str) -> Result<String> {
        Property::get_str(Self::get_props(), name, self)
    }

    fn get_u64(&self, name: &str) -> Result<u64> {
        Property::get_u64(Self::get_props(), name, self)
    }

    fn set_bool(&mut self, name: &str, value: bool) -> Result<()> {
        Property::set_bool(Self::get_props(), name, self, value)
    }

    fn set_i32(&mut self, name: &str, value: i32) -> Result<()> {
        Property::set_i32(Self::get_props(), name, self, value)
    }

    fn set_str(&mut self, name: &str, value: &str) -> Result<()> {
        Property::set_str(Self::get_props(), name, self, value)
    }

    fn set_u64(&mut self, name: &str, value: u64) -> Result<()> {
        Property::set_u64(Self::get_props(), name, self, value)
    }
}

macro_rules! properties {
    // Property definitions for the lazy_static vector
    (@prop $obj:ident mut $name:ident: $type:ty) => {
        paste::paste! {
            $crate::properties::Property::[<new_ $type>](
                stringify!($name),
                $obj::[<get_ $name>],
                $obj::[<set_ $name>])
        }
    };
    (@prop $obj:ident $name:ident: $type:ty) => {
        paste::paste! {
            $crate::properties::Property::[<new_ $type>](
                stringify!($name),
                $obj::[<get_ $name>],
                |_, _| { Err($crate::properties::error_read_only()) })
        }
    };
    (@prop $obj:ident fn $name:ident: $type:ty) => {
        properties!(@prop $obj $name: $type)
    };

    // Definitions of the configuration struct
    (@def $cfg:ident { } $($processed:tt)*) => {
        #[derive(Debug)]
        struct $cfg {
            $($processed)*
        }
    };
    (@def $cfg:ident { $prop:ident: str, $($rest:tt)* } $($processed:tt)*) => {
        properties!(@def $cfg { $($rest)* } $($processed)* $prop: String,);
    };
    (@def $cfg:ident { $prop:ident: $type:ident, $($rest:tt)* } $($processed:tt)*) => {
        properties!(@def $cfg { $($rest)* } $($processed)* $prop: $type,);
    };
    (@def $cfg:ident { mut $($rest:tt)* } $($processed:tt)*) => {
        properties!(@def $cfg { $($rest)* } $($processed)*);
    };
    (@def $cfg:ident { fn $prop:ident: $type:ident, $($rest:tt)* } $($processed:tt)*) => {
        properties!(@def $cfg { $($rest)* } $($processed)*);
    };

    // Generated property accessors
    {@acc $obj_props:ident $name:ident: str} => {
        paste::paste! {
            fn [<get_ $name>](&self) -> Result<String> {
                Ok(self.$obj_props.$name.clone())
            }
        }
    };
    {@acc $obj_props:ident $name:ident: $type:ty} => {
        paste::paste! {
            fn [<get_ $name>](&self) -> Result<$type> {
                Ok(self.$obj_props.$name)
            }
        }
    };
    {@acc $obj_props:ident mut $name:ident: $type:tt} => {
        properties!(@acc $obj_props $name: $type);
    };
    {@acc $obj_props:ident fn $name:ident: $type:ty} => {
    };

    // Putting everything together
    ($props_vec:ident: $cfg:ident for $obj:ident.$obj_props:ident {
        $($($prop:ident)+: $type:ident),*
    }) => {
        properties!(@def $cfg { $($($prop)+: $type),*, });
        lazy_static::lazy_static! {
            static ref $props_vec: Vec<$crate::properties::Property<$obj>> = vec![
                $(properties!(@prop $obj $($prop)+: $type)),*
            ];
        }

        impl $obj {
            $(properties!{@acc $obj_props $($prop)+: $type})*
        }

        impl PropertiesList for $obj {
            fn get_props<'a>() -> &'a Vec<Property<Self>> {
                &$props_vec
            }
        }
    };
}

pub(crate) use properties;
