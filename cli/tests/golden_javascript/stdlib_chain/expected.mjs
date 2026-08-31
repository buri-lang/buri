const $k0=[0,0];
function __cmd_x_main_buri$main(){
  const ctx_0=[[],[]];
  const trimmed_2=$str_trim('  the quick brown fox  ');
  const words_3=$str_split(trimmed_2,ctx_0,' ');
  const upper_6=$list_mapCtx(words_3,ctx_0,(c_4,w_5)=>$str_toUpper(w_5,c_4));
  const joined_7=$list_join(upper_6,ctx_0,'-');
  const self_10=$host_HostStdout_println(ctx_0[1],joined_7);
  let $t1;
  if(self_10[0]===0){
    $t1=0;
  }else if(self_10[0]===1){
    $t1=0;
  }else{
    $abort('no arm matched');
  }
  const text_14=String($str_len(trimmed_2))+' '+String($list_len(words_3))+' '+$str($str_contains(joined_7,'QUICK'));
  const self_15=$host_HostStdout_println(ctx_0[1],text_14);
  let $t3;
  if(self_15[0]===0){
    $t3=0;
  }else if(self_15[0]===1){
    $t3=0;
  }else{
    $abort('no arm matched');
  }
  const text_19=$str($str_startsWith(trimmed_2,'the'))+' '+$str($str_endsWith(trimmed_2,'fox'));
  const self_20=$host_HostStdout_println(ctx_0[1],text_19);
  let $t5;
  if(self_20[0]===0){
    $t5=0;
  }else if(self_20[0]===1){
    $t5=0;
  }else{
    $abort('no arm matched');
  }
  return $k0;
}
