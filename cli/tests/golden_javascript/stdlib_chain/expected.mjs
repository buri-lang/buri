const $k0=[[],[],[],[],[],[],[],[],[],[],[],[],[],[],[]];
const $k1=[0,0];
function __cmd_x_main_buri$main(){
  const ctx_1=[$k0[0],$k0[1]];
  const trimmed_3=$str_trim('  the quick brown fox  ');
  const words_4=$str_split(trimmed_3,ctx_1,' ');
  const upper_7=$list_mapCtx(words_4,ctx_1,(c_5,w_6)=>$str_toUpper(w_6,c_5));
  const joined_8=$list_join(upper_7,ctx_1,'-');
  const self_11=$host_HostStdout_println(ctx_1[1],joined_8);
  let $t1;
  if(self_11[0]===0){
    $t1=0;
  }else if(self_11[0]===1){
    $t1=0;
  }else{
    $abort('no arm matched');
  }
  const text_15=String($str_length(trimmed_3))+' '+String($list_length(words_4))+' '+$str($str_contains(joined_8,'QUICK'));
  const self_16=$host_HostStdout_println(ctx_1[1],text_15);
  let $t3;
  if(self_16[0]===0){
    $t3=0;
  }else if(self_16[0]===1){
    $t3=0;
  }else{
    $abort('no arm matched');
  }
  const text_20=$str($str_startsWith(trimmed_3,'the'))+' '+$str($str_endsWith(trimmed_3,'fox'));
  const self_21=$host_HostStdout_println(ctx_1[1],text_20);
  let $t5;
  if(self_21[0]===0){
    $t5=0;
  }else if(self_21[0]===1){
    $t5=0;
  }else{
    $abort('no arm matched');
  }
  return $k1;
}
