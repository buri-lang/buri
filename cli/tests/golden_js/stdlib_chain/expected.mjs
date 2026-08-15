const $k0=[0,0];
function __cmd_x_main$main(){
  const ctx_0=[[],[]];
  const trimmed_2=$str_trim('  the quick brown fox  ');
  const words_3=$str_split(trimmed_2,ctx_0,' ');
  const upper_6=$list_mapCtx(words_3,ctx_0,(c_4,w_5)=>$str_toUpper(w_5,c_4));
  const joined_7=$list_join(upper_6,ctx_0,'-');
  $host_HostStdout_println(ctx_0[1],[joined_7]);
  $host_HostStdout_println(ctx_0[1],[String($str_len(trimmed_2)),' ',String($list_len(words_3)),' ',$str_contains(joined_7,'QUICK')]);
  $host_HostStdout_println(ctx_0[1],[$str_startsWith(trimmed_2,'the'),' ',$str_endsWith(trimmed_2,'fox')]);
  return $k0;
}
