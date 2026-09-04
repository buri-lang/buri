const $k0=[0,0];
const $k1=[1n,2n];
const $k2=[3n,4n];
function __cmd_x_main_buri$main(){
  const ctx_0=[[],[]];
  const $t1=__cmd_x_main_buri$readTuple$u3rqgv(ctx_0,2n);
  const text_7=String($t1[0])+' '+String($t1[1]);
  const self_8=$host_HostStdout_println(ctx_0[1],text_7);
  let $t2;
  if(self_8[0]===0){
    $t2=0;
  }else if(self_8[0]===1){
    $t2=0;
  }else{
    $abort('no arm matched');
  }
  const $t4=__cmd_x_main_buri$readPair$u3rqgv(ctx_0,2n);
  const text_12=String($t4[0])+' '+String($t4[1]);
  const self_13=$host_HostStdout_println(ctx_0[1],text_12);
  let $t5;
  if(self_13[0]===0){
    $t5=0;
  }else if(self_13[0]===1){
    $t5=0;
  }else{
    $abort('no arm matched');
  }
  const whole_5=__cmd_x_main_buri$readTuple$u3rqgv(ctx_0,2n);
  const text_17=String(whole_5[0])+' '+String(whole_5[1]);
  const self_18=$host_HostStdout_println(ctx_0[1],text_17);
  let $t7;
  if(self_18[0]===0){
    $t7=0;
  }else if(self_18[0]===1){
    $t7=0;
  }else{
    $abort('no arm matched');
  }
  return $k0;
}
function __cmd_x_main_buri$readTuple$u3rqgv(ctx_0,depth_1){
  while(true){
    if(depth_1>0n){
      depth_1=depth_1-1n;
      continue;
    }else{
      const self_4=$host_HostStdout_println(ctx_0[1],'read a tuple');
      let $t1;
      if(self_4[0]===0){
        $t1=0;
      }else if(self_4[0]===1){
        $t1=0;
      }else{
        $abort('no arm matched');
      }
      return $k1;
    }
  }
}
function __cmd_x_main_buri$readPair$u3rqgv(ctx_0,depth_1){
  while(true){
    if(depth_1>0n){
      depth_1=depth_1-1n;
      continue;
    }else{
      const self_4=$host_HostStdout_println(ctx_0[1],'read a pair');
      let $t1;
      if(self_4[0]===0){
        $t1=0;
      }else if(self_4[0]===1){
        $t1=0;
      }else{
        $abort('no arm matched');
      }
      return $k2;
    }
  }
}
