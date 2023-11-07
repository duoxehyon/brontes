sol! (UniswapV3, "./abis/UniswapV3.json");
sol! (UniswapV2, "./abis/UniswapV2.json");
sol! (SushiSwapV2, "./abis/SushiSwapV2.json");


#[allow(non_camel_case_types)]
#[repr(u32)]
 pub enum StaticBindings {
   UniswapV3(UniswapV3_Enum),
   UniswapV2(UniswapV2_Enum),
   SushiSwapV2(SushiSwapV2_Enum),
}
impl StaticBindings {
 pub fn try_decode(&self, call_data: &[u8]) -> Result<StaticReturnBindings, alloy_sol_types::Error> {
     match self {
       StaticBindings::UniswapV3(_) => Ok(StaticReturnBindings::UniswapV3(UniswapV3_Enum::try_decode(call_data)?)),
       StaticBindings::UniswapV2(_) => Ok(StaticReturnBindings::UniswapV2(UniswapV2_Enum::try_decode(call_data)?)),
       StaticBindings::SushiSwapV2(_) => Ok(StaticReturnBindings::SushiSwapV2(SushiSwapV2_Enum::try_decode(call_data)?)),
_=> panic!("no binding match found")}
 }
}


#[allow(non_camel_case_types)]
#[repr(u32)]
 pub enum StaticReturnBindings {
   UniswapV3(UniswapV3::UniswapV3Calls),
   UniswapV2(UniswapV2::UniswapV2Calls),
   SushiSwapV2(SushiSwapV2::SushiSwapV2Calls),
}

#[allow(non_camel_case_types)]
#[repr(u32)]
 pub enum UniswapV3_Enum {
 None
}
impl_decode_sol!(UniswapV3_Enum, UniswapV3::UniswapV3Calls);



#[allow(non_camel_case_types)]
#[repr(u32)]
 pub enum UniswapV2_Enum {
 None
}
impl_decode_sol!(UniswapV2_Enum, UniswapV2::UniswapV2Calls);



#[allow(non_camel_case_types)]
#[repr(u32)]
 pub enum SushiSwapV2_Enum {
 None
}
impl_decode_sol!(SushiSwapV2_Enum, SushiSwapV2::SushiSwapV2Calls);

