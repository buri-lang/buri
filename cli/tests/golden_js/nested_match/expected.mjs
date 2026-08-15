function __cmd_x_main$main(){
  const ctx_0=[[],[]];
  core_host$HostStdout_println(ctx_0[1],[__cmd_x_main$render([0,2]),' ',__cmd_x_main$render([1,0,1])]);
  core_host$HostStdout_println(ctx_0[1],[__cmd_x_main$render([1,3,0]),' ',__cmd_x_main$render([2])]);
  return [0,0];
}
function __cmd_x_main$render(o_0){
  if(o_0[0]===0&&o_0[1]===0){
    return '1A';
  }else if(o_0[0]===0&&o_0[1]===1){
    return '1B';
  }else if(o_0[0]===0&&o_0[1]===2){
    return '1C';
  }else if(o_0[0]===0&&o_0[1]===3){
    return '1D';
  }else if(o_0[0]===1&&o_0[1]===0){
    return o_0[2]>0?'2A+':'2A-';
  }else if(o_0[0]===1){
    return '2*';
  }else if(o_0[0]===2){
    return '3';
  }else{
    $abort('no arm matched');
  }
}
function core_host$HostStdout_println(self_0,text_1){
  return $host_HostStdout_println(self_0,text_1);
}
