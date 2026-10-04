const $k0=[[],[],[],[],[],[],[],[],[],[],[],[],[],[],[]];
const $k1=[0,0];
function __cmd_x_main_buri$main(){
  const ctx_1=[$k0[0],$k0[1]];
  const fs_2=__cmd_x_main_buri$adders$9xug0c(ctx_1,0n,[]);
  const gs_3=__cmd_x_main_buri$scalers$9xug0c(ctx_1,7n,0n,[]);
  const text_10=String($list_length($list_map(fs_2,ctx_1,f_4=>f_4(100n))))+' '+String(__cmd_x_main_buri$sumTo(100n,0n));
  const self_11=$host_HostStdout_println(ctx_1[1],text_10);
  let $t1;
  if(self_11[0]===0){
    $t1=0;
  }else if(self_11[0]===1){
    $t1=0;
  }else{
    $abort('no arm matched');
  }
  const self_14=core_option$Option_map$g9y0aa($list_getFlat(fs_2,0n),f_5=>f_5(100n));
  let $t3;
  if(self_14!==void 0){
    $t3=self_14;
  }else if(self_14===void 0){
    $t3=-1n;
  }else{
    $abort('no arm matched');
  }
  const self_17=core_option$Option_map$g9y0aa($list_getFlat(fs_2,3n),f_6=>f_6(100n));
  let $t5;
  if(self_17!==void 0){
    $t5=self_17;
  }else if(self_17===void 0){
    $t5=-1n;
  }else{
    $abort('no arm matched');
  }
  const text_21=String($t3)+' '+String($t5);
  const self_22=$host_HostStdout_println(ctx_1[1],text_21);
  let $t7;
  if(self_22[0]===0){
    $t7=0;
  }else if(self_22[0]===1){
    $t7=0;
  }else{
    $abort('no arm matched');
  }
  const self_25=core_option$Option_map$g9y0aa($list_getFlat(gs_3,0n),g_7=>g_7(2n));
  let $t9;
  if(self_25!==void 0){
    $t9=self_25;
  }else if(self_25===void 0){
    $t9=-1n;
  }else{
    $abort('no arm matched');
  }
  const self_28=core_option$Option_map$g9y0aa($list_getFlat(gs_3,2n),g_8=>g_8(2n));
  let $t11;
  if(self_28!==void 0){
    $t11=self_28;
  }else if(self_28===void 0){
    $t11=-1n;
  }else{
    $abort('no arm matched');
  }
  const text_32=String($t9)+' '+String($t11);
  const self_33=$host_HostStdout_println(ctx_1[1],text_32);
  let $t13;
  if(self_33[0]===0){
    $t13=0;
  }else if(self_33[0]===1){
    $t13=0;
  }else{
    $abort('no arm matched');
  }
  return $k1;
}
function __cmd_x_main_buri$adders$9xug0c(ctx_0,i_loop_4,acc_2){
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
function __cmd_x_main_buri$scalers$9xug0c(ctx_0,k_1,i_2,acc_3){
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
function core_option$Option_map$g9y0aa(self_0,transform_1){
  if(self_0!==void 0){
    return transform_1(self_0);
  }else if(self_0===void 0){
    return void 0;
  }else{
    $abort('no arm matched');
  }
}
