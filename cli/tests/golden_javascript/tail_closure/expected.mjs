const $k0=[0,0];
function __cmd_x_main$main(){
  const ctx_0=[[],[]];
  const fs_1=__cmd_x_main$adders$u3rqgv(ctx_0,0,[]);
  const gs_2=__cmd_x_main$scalers$u3rqgv(ctx_0,7,0,[]);
  $host_HostStdout_println(ctx_0[1],String($list_len($list_map(fs_1,ctx_0,f_3=>f_3(100))))+' '+String(__cmd_x_main$sumTo(100,0)));
  const $t1=core_option$Option_map$g9y0aa($list_get(fs_1,0),f_4=>f_4(100));
  const $t2=core_option$Option_map$g9y0aa($list_get(fs_1,3),f_5=>f_5(100));
  $host_HostStdout_println(ctx_0[1],String($t1!==void 0?$t1:-1)+' '+String($t2!==void 0?$t2:-1));
  const $t4=core_option$Option_map$g9y0aa($list_get(gs_2,0),g_6=>g_6(2));
  const $t5=core_option$Option_map$g9y0aa($list_get(gs_2,2),g_7=>g_7(2));
  $host_HostStdout_println(ctx_0[1],String($t4!==void 0?$t4:-1)+' '+String($t5!==void 0?$t5:-1));
  return $k0;
}
function __cmd_x_main$adders$u3rqgv(ctx_0,$p1,acc_2){
  while(true){
    const i_1=$p1;
    if(i_1>=4){
      return acc_2;
    }else{
      acc_2=$list_push(acc_2,ctx_0,x_3=>x_3+i_1);
      $p1=i_1+1;
      continue;
    }
  }
}
function __cmd_x_main$scalers$u3rqgv(ctx_0,k_1,i_2,acc_3){
  while(true){
    if(i_2>=3){
      return acc_3;
    }else{
      const $t1=i_2+1;
      acc_3=$list_push(acc_3,ctx_0,x_4=>x_4*k_1);
      i_2=$t1;
      continue;
    }
  }
}
function __cmd_x_main$sumTo(n_0,acc_1){
  while(true){
    if(n_0===0){
      return acc_1;
    }else{
      const $t1=n_0-1;
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
