#![deprecated]
#![allow(dead_code, deprecated)]

use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct ApiResponse<T> {
    pub code: u8,
    pub message: &'static str,
    pub data: T,
}

impl<T> ApiResponse<T> {
    pub fn ok(data: T) -> Self {
        Self {
            code: 1,
            message: "success",
            data,
        }
    }
    // pub fn err() -> Self {
    //     Self {
    //         code: 0,
    //         message: "error",
    //         data: ,
    //     }
    // }
}
