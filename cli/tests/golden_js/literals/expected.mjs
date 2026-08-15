function __cmd_x_main$main(){
  const ctx_0=[[],[]];
  const a_1=__cmd_x_main$defaultConfig();
  const b_2=__cmd_x_main$defaultConfig();
  core_host$HostStdout_println(ctx_0[1],[String(a_1[0]+b_2[0]),' ',a_1[1]]);
  core_host$HostStdout_println(ctx_0[1],[String(__cmd_x_main$severity(__cmd_x_main$quiet())),' ',String(__cmd_x_main$severity([3,'x']))]);
  core_host$HostStdout_println(ctx_0[1],[String(__cmd_x_main$origin()[0]),' ',String(core_list$len$1bogxm(__cmd_x_main$primes())),' ',String(core_list$sum(__cmd_x_main$primes()))]);
  return [0,0];
}
function __cmd_x_main$defaultConfig(){
  return [3,'default'];
}
function core_host$HostStdout_println(self_0,text_1){
  return $host_HostStdout_println(self_0,text_1);
}
function __cmd_x_main$quiet(){
  return [1];
}
function __cmd_x_main$severity(l_0){
  if(l_0[0]===0){
    return 0;
  }else if(l_0[0]===1){
    return 1;
  }else if(l_0[0]===2){
    return 2;
  }else if(l_0[0]===3){
    return 3;
  }else{
    $abort('no arm matched');
  }
}
function __cmd_x_main$origin(){
  return [0,0];
}
function __cmd_x_main$primes(){
  return [2,3,5,7,11];
}
function core_list$len$1bogxm(self_0){
  return $list_len(self_0);
}
function core_list$sum(self_0){
  return core_list$fold$71n5xt(self_0,(acc_1,x_2)=>acc_1+x_2,0);
}
function core_list$fold$71n5xt(self_0,f_1,init_2){
  return $list_fold(self_0,f_1,init_2);
}
