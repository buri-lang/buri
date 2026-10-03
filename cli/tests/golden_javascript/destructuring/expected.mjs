const $k0=[[],[],[],[],[],[],[],[],[],[],[],[],[],[],[]];
const $k1=[0,0];
const $k2=[1n,2n];
const $k3=[3n,4n];
function __cmd_x_main_buri$main(){
  const ctx_1=[$k0[0],$k0[1]];
  const $t1=__cmd_x_main_buri$readTuple$u3rqgv(ctx_1,2n);
  const text_8=String($t1[0])+' '+String($t1[1]);
  const self_9=$host_HostStdout_println(ctx_1[1],text_8);
  let $t2;
  if(self_9[0]===0){
    $t2=0;
  }else if(self_9[0]===1){
    $t2=0;
  }else{
    $abort('no arm matched');
  }
  const $t4=__cmd_x_main_buri$readPair$u3rqgv(ctx_1,2n);
  const text_13=String($t4[0])+' '+String($t4[1]);
  const self_14=$host_HostStdout_println(ctx_1[1],text_13);
  let $t5;
  if(self_14[0]===0){
    $t5=0;
  }else if(self_14[0]===1){
    $t5=0;
  }else{
    $abort('no arm matched');
  }
  const whole_6=__cmd_x_main_buri$readTuple$u3rqgv(ctx_1,2n);
  const text_18=String(whole_6[0])+' '+String(whole_6[1]);
  const self_19=$host_HostStdout_println(ctx_1[1],text_18);
  let $t7;
  if(self_19[0]===0){
    $t7=0;
  }else if(self_19[0]===1){
    $t7=0;
  }else{
    $abort('no arm matched');
  }
  return $k1;
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
      return $k2;
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
      return $k3;
    }
  }
}
