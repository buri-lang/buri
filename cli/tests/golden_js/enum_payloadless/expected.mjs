function __cmd_x_main$main(){
  const ctx_0=[[],[]];
  core_host$HostStdout_println(ctx_0[1],[__cmd_x_main$name(0),' ',__cmd_x_main$name(5)]);
  core_host$HostStdout_println(ctx_0[1],[__cmd_x_main$name(2),' ',__cmd_x_main$name(4)]);
  return [0,0];
}
function __cmd_x_main$name(c_0){
  if(c_0===0){
    return 'red';
  }else if(c_0===1){
    return 'green';
  }else if(c_0===2){
    return 'blue';
  }else if(c_0===3){
    return 'cyan';
  }else if(c_0===4){
    return 'magenta';
  }else if(c_0===5){
    return 'yellow';
  }else{
    $abort('no arm matched');
  }
}
function core_host$HostStdout_println(self_0,text_1){
  return $host_HostStdout_println(self_0,text_1);
}
