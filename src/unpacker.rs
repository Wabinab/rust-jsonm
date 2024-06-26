extern crate regex;
extern crate serde;
extern crate serde_json;

use self::regex::Regex;
use self::serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::vec::Vec;

//const OLD_MESSAGE: i32 = -99;
const MIN_DICT_INDEX: u64 = 3;
const TYPE_ARRAY: i64 = 0;
const TYPE_VALUE: i64 = 1;
const TYPE_STRING: i64 = 2;
const MAX_PACK_COMPLEX_OBJECT_SIZE: usize = 12;

#[derive(Default, Debug)]
pub struct Unpacker {
    // dict: HashMap<u64, String>,
    dict: HashMap<u64, Value>,
    dict_index: u64,
    sequence_id: i64,
    max_dict_size: u64,
    pending_unpacks: Vec<i32>,
}

#[derive(Debug, Clone)]
pub struct UnpackerError {
    pub cause: String,
}

impl fmt::Display for UnpackerError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "UnpackerError")
    }
}

impl Error for UnpackerError {
    fn description(&self) -> &str {
        "Unpacker Error"
    }

    fn cause(&self) -> Option<&dyn Error> {
        None
    }
}

impl Unpacker {
    pub fn new() -> Unpacker {
        Unpacker {
            sequence_id: -1,
            max_dict_size: 2000,
            dict_index: MIN_DICT_INDEX,
            ..Default::default()
        }
    }

    /// Unpack an packed object to its original input.
    pub fn unpack<T>(&mut self, packed_object: &Value) -> Result<T, UnpackerError>
    where
        for<'de> T: Deserialize<'de>,
    {
        if packed_object.is_null() {
            return match serde_json::from_value(Value::Null) {
                Ok(v) => Ok(v),
                Err(_err) => Err(UnpackerError {
                    cause: "wrong end type for Value::Null, use Value type instead".to_owned(),
                }),
            };
        };

        let packed_arr = match packed_object.as_array() {
            Some(packed_arr) => packed_arr,
            None => {
                return Err(UnpackerError {
                    cause: "packed value expected".to_owned(),
                })
            }
        };

        if !packed_arr[packed_arr.len() - 1].is_number() {
            return Err(UnpackerError {
                cause: "packed value expected".to_owned(),
            });
        };

        let value = &packed_arr[packed_arr.len() - 1];
        let remote_sequence_id = match value.as_i64() {
            Some(v) => v,
            None => {
                return Err(UnpackerError {
                    cause: "packed value expected".to_owned(),
                })
            }
        };

        if remote_sequence_id == 0 {
            self.dict_index = MIN_DICT_INDEX;
        } else if remote_sequence_id != (self.sequence_id + 1) {
            return Err(UnpackerError {
                cause: "message unpacked out of sequence or already unpacked".to_owned(),
            });
        };

        self.sequence_id = remote_sequence_id;
        let unpacked = match self.unpack_object(&json!(packed_arr[..(packed_arr.len() - 1)])) {
            Ok(result) => result,
            Err(err) => return Err(err),
        };

        let result: T = match serde_json::from_value(unpacked) {
            Ok(result) => result,
            Err(_err) => {
                return Err(UnpackerError {
                    cause: "unable to unpack to specific type".to_owned(),
                })
            }
        };
        Ok(result)
    }

    /// Unpack an object to a string.
    pub fn unpack_string(&mut self, packed_object: &Value) -> Result<String, UnpackerError> {
        match packed_object.as_array() {
            Some(arr) => {
                if arr[0] == TYPE_STRING {
                    return self.unpack(packed_object);
                }

                match self.unpack::<Value>(packed_object) {
                    Ok(s) => Ok(s.to_string()),
                    Err(err) => Err(err),
                }
            }
            None => match self.unpack::<Value>(packed_object) {
                Ok(s) => Ok(s.to_string()),
                Err(err) => Err(err),
            },
        }
    }

    fn unpack_object(&mut self, packed_object: &Value) -> Result<Value, UnpackerError> {
        if packed_object.is_null() {
            return Ok(Value::Null);
        };

        if !packed_object.is_array() {
            return self.unpack_value(packed_object);
        }

        // if packed_object == &json!([0]) {
        //     return Ok(json!({}));
        // }

        let packed_array = match packed_object.as_array() {
            Some(packed_array) => packed_array,
            None => {
                return Err(UnpackerError {
                    cause: "wrong packed object".to_owned(),
                })
            }
        };

        if packed_array.len() == 0 { return Ok(json!({})); }
        let type_value = &packed_array[0];
        let type_id = match type_value.as_i64() {
            Some(i) => i,
            None => -1,
        };

        if type_id == TYPE_ARRAY {
            return packed_array[1..]
                .iter()
                .map(|v| self.unpack_object(&v))
                .collect();
        }
        if type_id == TYPE_STRING {
            return match packed_array[1..]
                .iter()
                .map(|v| self.unpack_object(&v))
                .collect::<Result<Value, _>>()
            {
                Ok(arr) => {
                    let vec = match arr.as_array() {
                        Some(a) => a,
                        None => {
                            return Err(UnpackerError {
                                cause: "expected array, got something else".to_owned(),
                            })
                        }
                    };
                    Ok(json!(vec.iter().fold("".to_owned(), |acc, x| {
                        if acc.is_empty() {
                            x.as_str().unwrap().to_owned()
                        } else {
                            acc + "\n" + x.as_str().unwrap()
                        }
                    })))
                }
                Err(err) => return Err(err),
            };
        }
        if type_id == TYPE_VALUE {
            return self.unpack_value(&packed_array[1]);
        }

        let mut contains_unmemoised = false;
        let mut processed_object: Vec<Value> = Vec::new();
        for item in packed_array {
            if item.is_object() || item.is_array() {
                let value = match self.unpack_object(&item) {
                    Ok(v) => v,
                    Err(err) => return Err(err),
                };
                contains_unmemoised = true;
                processed_object.push(value);
            } else {
                if !item.is_number() {
                    contains_unmemoised = true;
                }
                let value = match self.unpack_value(&item) {
                    Ok(v) => v,
                    Err(err) => return Err(err),
                };

                processed_object.push(value);
            }
        }

        let mut result: HashMap<String, Value> = HashMap::new();
        let key_count = processed_object.len() / 2;
        for i in 0..key_count {
            let key_value = &processed_object[i];
            let key = match processed_object[i].as_str() {
                Some(s) => s.to_string(),
                None => key_value.to_string(),
            };
            result.insert(key, processed_object[i + key_count].clone());
        }

        let json_result = json!(result);
        if !contains_unmemoised && packed_array.len() <= MAX_PACK_COMPLEX_OBJECT_SIZE {
            // self.add_to_dict_str(&json_result.to_string());
            self.add_to_dict(&json_result);
        }

        Ok(json_result)
    }

    fn unpack_value(&mut self, packed_object: &Value) -> Result<Value, UnpackerError> {
        if packed_object.is_number() {
            return match packed_object.as_i64() {
                Some(v) => {
                    if v < 0 {
                        return Ok(json!(v * -1));
                    }
                    let index = packed_object.as_u64().unwrap();
                    let string = match self.dict.get(&index) {
                        Some(s) => s,
                        None => {
                            println!("Wrong Index: {:?}", index);
                            // println!("Index: {:?}\nDict: {:#?}", index, self.dict);
                            return Err(UnpackerError {
                                cause: "no stored value".to_owned(),
                            })
                            // return Ok(json!(null));
                        }
                    };
                    let json = string.to_owned();
                    // let json: serde_json::Value = match serde_json::from_str(&string) {
                    //     Ok(parsed) => parsed,
                    //     Err(_err) => json!(string),
                    // };

                    Ok(json)
                }
                None => Err(UnpackerError {
                    cause: "unknown".to_owned(),
                }),
            };
        };

        if packed_object.is_string() {
            let string = match packed_object.as_str() {
                Some(s) => s,
                None => {
                    return Err(UnpackerError {
                        cause: "unknown".to_owned(),
                    })
                }
            };

            let re = Regex::new(r"^-?[0-9]+\.").unwrap();
            if re.is_match(string) {
                let _p: Value = match string.parse::<f64>() {
                    Ok(parse_number) => {
                        // self.add_to_dict_str(string);
                        self.add_to_dict(&json!(parse_number));
                        return Ok(json!(parse_number));
                    }
                    Err(_err) => Value::Null,
                };
            };

            let re = Regex::new(r"^-?[0-9\.]").unwrap();
            if re.is_match(string) {
                let _p: Value = match string.parse::<i64>() {
                    Ok(parse_number) => {
                        // self.add_to_dict_str(string);
                        self.add_to_dict(&json!(parse_number));
                        return Ok(json!(parse_number));
                    }
                    Err(_err) => Value::Null,
                };
            };

            let value = if !string.is_empty() && &string[0..1] == "~" {
                &string[1..]
            } else {
                string
            };

            // self.add_to_dict_str(&json!(value).to_string());
            self.add_to_dict(&json!(value));
            return Ok(json!(value));
        }

        self.add_to_dict(packed_object);
        Ok(json!(packed_object))
    }

    // fn add_to_dict_str(&mut self, str_value: &str) {
    //     // println!("Index: {}\tValue: {}", self.dict_index, str_value.to_owned());
    //     self.dict.insert(self.dict_index, json!(str_value.to_owned()));
    //     self.dict_index += 1;
    //     if self.dict_index >= (self.max_dict_size + MIN_DICT_INDEX) {
    //         self.dict_index = MIN_DICT_INDEX;
    //     }
    // }

    fn add_to_dict(&mut self, value: &Value) {
        // println!("Index: {}\tValue: {}", self.dict_index, value.to_owned());
        self.dict.insert(self.dict_index, value.to_owned());
        self.dict_index += 1;
        if self.dict_index >= (self.max_dict_size + MIN_DICT_INDEX) {
            self.dict_index = MIN_DICT_INDEX;
        }
    }

    /// Set the maximum dictionary size. Must match the dictionary size used by the packer.
    /// Default - 2000.
    pub fn set_max_dict_size(&mut self, value: u64) {
        self.max_dict_size = value;
    }
}
