const $k0=[[],[],[],[],[],[],[],[],[],[],[],[],[]];
const $k1=[0,0];
function __cmd_x_main_buri$main(){
  const ctx_1=[$k0[0],$k0[1],$k0[10],$k0[11]];
  const count_2=[$host_HostUi_signal(ctx_1[2],0n)];
  const id_13=$host_HostUi_memo(ctx_1[2],s_3=>$ui_effect_Scope_read(s_3,count_2[0])*2n);
  const doubled_4=[2,scope_14=>$ui_effect_Scope_read(scope_14,id_13)];
  const run_22=s_5=>{
    let $t1;
    if(doubled_4[0]===0){
      $t1=doubled_4[1];
    }else if(doubled_4[0]===1){
      $t1=ui_signal$Signal_get$io0sz4(doubled_4[1],s_5);
    }else if(doubled_4[0]===2){
      $t1=doubled_4[1](s_5);
    }else{
      $abort('no arm matched');
    }
    return 0;
  };
  $host_HostUi_watch(ctx_1[2],run_22);
  const text_26='count '+String($host_HostWatch_read(ctx_1[3],count_2[0]));
  const self_27=$host_HostStdout_println(ctx_1[1],text_26);
  let $t3;
  if(self_27[0]===0){
    $t3=0;
  }else if(self_27[0]===1){
    $t3=0;
  }else{
    $abort('no arm matched');
  }
  $host_HostUi_write(ctx_1[2],count_2[0],(n_6=>n_6+1n)($host_HostUi_read(ctx_1[2],count_2[0])));
  const text_36='count '+String($host_HostWatch_read(ctx_1[3],count_2[0]));
  const self_37=$host_HostStdout_println(ctx_1[1],text_36);
  let $t5;
  if(self_37[0]===0){
    $t5=0;
  }else if(self_37[0]===1){
    $t5=0;
  }else{
    $abort('no arm matched');
  }
  $host_HostUi_write(ctx_1[2],count_2[0],20n);
  const text_46='count '+String($host_HostWatch_read(ctx_1[3],count_2[0]));
  const self_47=$host_HostStdout_println(ctx_1[1],text_46);
  let $t7;
  if(self_47[0]===0){
    $t7=0;
  }else if(self_47[0]===1){
    $t7=0;
  }else{
    $abort('no arm matched');
  }
  return $k1;
}
function ui_signal$Signal_get$io0sz4(self_0,ctx_1){
  return $ui_effect_Scope_read(ctx_1,self_0[0]);
}
