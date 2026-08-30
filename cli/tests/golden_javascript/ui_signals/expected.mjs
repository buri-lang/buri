const $k0=[0,0];
function __cmd_x_main_buri$main(){
  const ctx_0=[[],[],[],[]];
  const count_1=[$host_HostUi_signal(ctx_0[2],0n)];
  const id_12=$host_HostUi_memo(ctx_0[2],s_2=>$ui_effect_Scope_read(s_2,count_1[0])*2n);
  const doubled_3=[2,scope_13=>$ui_effect_Scope_read(scope_13,id_12)];
  const run_21=s_4=>{
    let $t1;
    if(doubled_3[0]===0){
      $t1=doubled_3[1];
    }else if(doubled_3[0]===1){
      $t1=$ui_effect_Scope_read(s_4,doubled_3[1][0]);
    }else if(doubled_3[0]===2){
      $t1=doubled_3[1](s_4);
    }else{
      $abort('no arm matched');
    }
    return 0;
  };
  $host_HostUi_watch(ctx_0[2],run_21);
  const text_25='count '+String($host_HostWatch_read(ctx_0[3],count_1[0]));
  $host_HostStdout_println(ctx_0[1],text_25);
  $host_HostUi_write(ctx_0[2],count_1[0],(n_5=>n_5+1n)($host_HostUi_read(ctx_0[2],count_1[0])));
  const text_32='count '+String($host_HostWatch_read(ctx_0[3],count_1[0]));
  $host_HostStdout_println(ctx_0[1],text_32);
  $host_HostUi_write(ctx_0[2],count_1[0],20n);
  const text_39='count '+String($host_HostWatch_read(ctx_0[3],count_1[0]));
  $host_HostStdout_println(ctx_0[1],text_39);
  return $k0;
}
