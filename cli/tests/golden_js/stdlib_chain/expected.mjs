function __cmd_x_main$main(){
  const ctx_0=[[],[]];
  const trimmed_2=core_str$Str_trim('  the quick brown fox  ');
  const words_3=core_str$Str_split$72mdf3(trimmed_2,ctx_0,' ');
  const upper_6=core_list$mapCtx$j169k2(words_3,ctx_0,(c_4,w_5)=>core_str$Str_toUpper$72mdf3(w_5,c_4));
  const joined_7=core_list$join$72mdf3(upper_6,ctx_0,'-');
  core_host$HostStdout_println(ctx_0[1],[joined_7]);
  core_host$HostStdout_println(ctx_0[1],[String(core_str$Str_len(trimmed_2)),' ',String(core_list$len$ea3yj9(words_3)),' ',core_str$Str_contains(joined_7,'QUICK')]);
  core_host$HostStdout_println(ctx_0[1],[core_str$Str_startsWith(trimmed_2,'the'),' ',core_str$Str_endsWith(trimmed_2,'fox')]);
  return [0,0];
}
function core_str$Str_trim(self_0){
  return $str_trim(self_0);
}
function core_str$Str_split$72mdf3(self_0,ctx_1,separator_2){
  return $str_split(self_0,ctx_1,separator_2);
}
function core_str$Str_toUpper$72mdf3(self_0,ctx_1){
  return $str_toUpper(self_0,ctx_1);
}
function core_list$mapCtx$j169k2(self_0,ctx_1,f_2){
  return $list_mapCtx(self_0,ctx_1,f_2);
}
function core_list$join$72mdf3(self_0,ctx_1,separator_2){
  return $list_join(self_0,ctx_1,separator_2);
}
function core_host$HostStdout_println(self_0,text_1){
  return $host_HostStdout_println(self_0,text_1);
}
function core_str$Str_len(self_0){
  return $str_len(self_0);
}
function core_list$len$ea3yj9(self_0){
  return $list_len(self_0);
}
function core_str$Str_contains(self_0,needle_1){
  return $str_contains(self_0,needle_1);
}
function core_str$Str_startsWith(self_0,prefix_1){
  return $str_startsWith(self_0,prefix_1);
}
function core_str$Str_endsWith(self_0,suffix_1){
  return $str_endsWith(self_0,suffix_1);
}
