const $k0=[[],[],[],[],[],[],[],[],[],[],[],[],[],[],[]];
const $k1=[3n,1n,4n,1n,5n];
const $k2=[0,0];
function __cmd_x_main_buri$main(){
  const ctx_1=[$k0[0],$k0[1]];
  const text_4=$str(__cmd_x_main_buri$allBelow($k1,10n,0n))+' '+$str(__cmd_x_main_buri$allBelow($k1,4n,0n));
  const self_5=$host_HostStdout_println(ctx_1[1],text_4);
  let $t1;
  if(self_5[0]===0){
    $t1=0;
  }else if(self_5[0]===1){
    $t1=0;
  }else{
    $abort('no arm matched');
  }
  const text_9=$str(__cmd_x_main_buri$anyAtLeast($k1,5n,0n))+' '+$str(__cmd_x_main_buri$anyAtLeast($k1,9n,0n));
  const self_10=$host_HostStdout_println(ctx_1[1],text_9);
  let $t3;
  if(self_10[0]===0){
    $t3=0;
  }else if(self_10[0]===1){
    $t3=0;
  }else{
    $abort('no arm matched');
  }
  const text_14=$str(__cmd_x_main_buri$bothSmall(1n,2n))+' '+$str(__cmd_x_main_buri$bothSmall(1n,20n));
  const self_15=$host_HostStdout_println(ctx_1[1],text_14);
  let $t5;
  if(self_15[0]===0){
    $t5=0;
  }else if(self_15[0]===1){
    $t5=0;
  }else{
    $abort('no arm matched');
  }
  return $k2;
}
function __cmd_x_main_buri$allBelow(xs_0,limit_1,i_2){
  while(true){
    if(i_2>=$list_length(xs_0)){
      return true;
    }else{
      const self_3=$list_getFlat(xs_0,i_2);
      let $t1;
      if(self_3!==void 0){
        $t1=self_3;
      }else if(self_3===void 0){
        $t1=0n;
      }else{
        $abort('no arm matched');
      }
      if($t1<limit_1){
        i_2=i_2+1n;
        continue;
      }else{
        return false;
      }
    }
  }
}
function __cmd_x_main_buri$anyAtLeast(xs_0,limit_1,i_2){
  while(true){
    if(i_2>=$list_length(xs_0)){
      return false;
    }else{
      const self_3=$list_getFlat(xs_0,i_2);
      let $t1;
      if(self_3!==void 0){
        $t1=self_3;
      }else if(self_3===void 0){
        $t1=0n;
      }else{
        $abort('no arm matched');
      }
      if($t1>=limit_1){
        return true;
      }else{
        i_2=i_2+1n;
        continue;
      }
    }
  }
}
function __cmd_x_main_buri$bothSmall(a_0,b_1){
  return a_0<10n&&b_1<10n;
}
