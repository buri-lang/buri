const $k0=[0,0];
function __cmd_x_main_buri$main(){
  const ctx_0=[[],[]];
  const fs_1=__cmd_x_main_buri$adders$u3rqgv(ctx_0,0n,[]);
  const gs_2=__cmd_x_main_buri$scalers$u3rqgv(ctx_0,7n,0n,[]);
  const text_9=String($list_len($list_map(fs_1,ctx_0,f_3=>f_3(100n))))+' '+String(__cmd_x_main_buri$sumTo(100n,0n));
  const self_10=$host_HostStdout_println(ctx_0[1],text_9);
  let $t1;
  if(self_10[0]===0){
    $t1=0;
  }else if(self_10[0]===1){
    $t1=0;
  }else{
    $abort('no arm matched');
  }
  const $t3=core_option$Option_map$g9y0aa($list_get(fs_1,0n),f_4=>f_4(100n));
  const $t4=core_option$Option_map$g9y0aa($list_get(fs_1,3n),f_5=>f_5(100n));
  const text_14=String($t3!==void 0?$t3:-1n)+' '+String($t4!==void 0?$t4:-1n);
  const self_15=$host_HostStdout_println(ctx_0[1],text_14);
  let $t5;
  if(self_15[0]===0){
    $t5=0;
  }else if(self_15[0]===1){
    $t5=0;
  }else{
    $abort('no arm matched');
  }
  const $t7=core_option$Option_map$g9y0aa($list_get(gs_2,0n),g_6=>g_6(2n));
  const $t8=core_option$Option_map$g9y0aa($list_get(gs_2,2n),g_7=>g_7(2n));
  const text_19=String($t7!==void 0?$t7:-1n)+' '+String($t8!==void 0?$t8:-1n);
  const self_20=$host_HostStdout_println(ctx_0[1],text_19);
  let $t9;
  if(self_20[0]===0){
    $t9=0;
  }else if(self_20[0]===1){
    $t9=0;
  }else{
    $abort('no arm matched');
  }
  return $k0;
}
function __cmd_x_main_buri$adders$u3rqgv(ctx_0,i_loop_4,acc_2){
  while(true){
    const i_1=i_loop_4;
    if(i_1>=4n){
      return acc_2;
    }else{
      acc_2=$list_push(acc_2,ctx_0,x_3=>x_3+i_1);
      i_loop_4=i_1+1n;
      continue;
    }
  }
}
function __cmd_x_main_buri$scalers$u3rqgv(ctx_0,k_1,i_2,acc_3){
  while(true){
    if(i_2>=3n){
      return acc_3;
    }else{
      const $t1=i_2+1n;
      acc_3=$list_push(acc_3,ctx_0,x_4=>x_4*k_1);
      i_2=$t1;
      continue;
    }
  }
}
function __cmd_x_main_buri$sumTo(n_0,acc_1){
  while(true){
    if(n_0===0n){
      return acc_1;
    }else{
      const $t1=n_0-1n;
      acc_1=acc_1+n_0;
      n_0=$t1;
      continue;
    }
  }
}
function core_option$Option_map$g9y0aa(self_0,f_1){
  if(self_0!==void 0){
    return f_1(self_0);
  }else if(self_0===void 0){
    return void 0;
  }else{
    $abort('no arm matched');
  }
}
