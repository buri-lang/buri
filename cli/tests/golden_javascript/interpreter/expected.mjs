const $k0=[0,2n];
const $k1=[1];
const $k2=[0,3n];
const $k3=[3];
const $k4=[0,4n];
const $k5=[2];
const $k6=[5];
const $k7=[0,10n];
const $k8=[4];
const $k9=[0,5n];
const $k10=[6];
const $k11=[$k0,$k1,$k2,$k3,$k4,$k5,$k6,$k7,$k8,$k9,$k10];
const $k12=[0,0];
const $k13=[1,$k5];
const $k14=[7];
const $k15=[1,'expected )'];
const $k16=[1,$k15];
const $k17=[0,0n];
const $k18=[1,'expected a value'];
const $k19=[1,$k18];
function __cmd_x_main_buri$main(){
  const parsed_2=__cmd_x_main_buri$parseSum([$k11,0n]);
  let $t1;
  if(parsed_2[0]===0){
    const pair_3=parsed_2[1];
    $share(pair_3);
    const $t4=__cmd_x_main_buri$eval(pair_3[0]);
    if($t4[0]===0){
      $t1='value '+String($t4[1])+' depth '+String(__cmd_x_main_buri$depth(pair_3[0]));
    }else if($t4[0]===1){
      let $t5;
      const $t6=$t4[1];
      switch($t6[0]){
        case 0:
        case 1:
          {
            $t5=$t6[1];
          }
          break;
        case 2:
          {
            $t5='division by zero';
          }
          break;
        case 3:
          {
            $t5='trailing input';
          }
          break;
        default:
          {
            $abort('no arm matched');
          }
          break;
      }
      $t1='eval error '+$t5;
    }else{
      $abort('no arm matched');
    }
  }else if(parsed_2[0]===1){
    let $t7;
    const $t8=parsed_2[1];
    switch($t8[0]){
      case 0:
      case 1:
        {
          $t7=$t8[1];
        }
        break;
      case 2:
        {
          $t7='division by zero';
        }
        break;
      case 3:
        {
          $t7='trailing input';
        }
        break;
      default:
        {
          $abort('no arm matched');
        }
        break;
    }
    $t1='parse error '+$t7;
  }else{
    $abort('no arm matched');
  }
  const text_8=$t1;
  const self_9=$host_HostStdout_println([[],[]][1],text_8);
  let $t9;
  if(self_9[0]===0){
    $t9=0;
  }else if(self_9[0]===1){
    $t9=0;
  }else{
    $abort('no arm matched');
  }
  return $k12;
}
function __cmd_x_main_buri$parseSum(c_0){
  const $t1=__cmd_x_main_buri$parseProduct(c_0);
  if($t1[0]!==0){
    return $t1;
  }
  const first_1=$t1[1];
  return __cmd_x_main_buri$parseSumFrom(first_1[0],$fromShared(first_1,first_1[1]));
}
function __cmd_x_main_buri$eval(e_0){
  switch(e_0[0]){
    case 0:
      {
        return [0,e_0[1]];
      }
    case 1:
      {
        const $t2=__cmd_x_main_buri$eval(e_0[1]);
        if($t2[0]!==0){
          return $t2;
        }
        const $t3=__cmd_x_main_buri$eval(e_0[2]);
        if($t3[0]!==0){
          return $t3;
        }
        return [0,$t2[1]+$t3[1]];
      }
    case 2:
      {
        const $t5=__cmd_x_main_buri$eval(e_0[1]);
        if($t5[0]!==0){
          return $t5;
        }
        const $t6=__cmd_x_main_buri$eval(e_0[2]);
        if($t6[0]!==0){
          return $t6;
        }
        return [0,$t5[1]-$t6[1]];
      }
    case 3:
      {
        const $t8=__cmd_x_main_buri$eval(e_0[1]);
        if($t8[0]!==0){
          return $t8;
        }
        const $t9=__cmd_x_main_buri$eval(e_0[2]);
        if($t9[0]!==0){
          return $t9;
        }
        return [0,$t8[1]*$t9[1]];
      }
    case 4:
      {
        const $t11=__cmd_x_main_buri$eval(e_0[2]);
        if($t11[0]!==0){
          return $t11;
        }
        const d_10=$t11[1];
        if(d_10===0n){
          return $k13;
        }else{
          const $t12=__cmd_x_main_buri$eval(e_0[1]);
          if($t12[0]!==0){
            return $t12;
          }
          return [0,$divb($t12[1],d_10)];
        }
      }
      break;
    default:
      {
        $abort('no arm matched');
      }
      break;
  }
}
function __cmd_x_main_buri$depth(e_0){
  switch(e_0[0]){
    case 0:
      {
        return 1n;
      }
    case 1:
      {
        const a_9=__cmd_x_main_buri$depth(e_0[1]);
        const b_10=__cmd_x_main_buri$depth(e_0[2]);
        return 1n+(a_9>b_10?a_9:b_10);
      }
    case 2:
      {
        const a_11=__cmd_x_main_buri$depth(e_0[1]);
        const b_12=__cmd_x_main_buri$depth(e_0[2]);
        return 1n+(a_11>b_12?a_11:b_12);
      }
    case 3:
      {
        const a_13=__cmd_x_main_buri$depth(e_0[1]);
        const b_14=__cmd_x_main_buri$depth(e_0[2]);
        return 1n+(a_13>b_14?a_13:b_14);
      }
    case 4:
      {
        const a_15=__cmd_x_main_buri$depth(e_0[1]);
        const b_16=__cmd_x_main_buri$depth(e_0[2]);
        return 1n+(a_15>b_16?a_15:b_16);
      }
    default:
      {
        $abort('no arm matched');
      }
      break;
  }
}
function __cmd_x_main_buri$parseProduct(c_0){
  const $t1=__cmd_x_main_buri$parsePrimary(c_0);
  if($t1[0]!==0){
    return $t1;
  }
  const first_1=$t1[1];
  return __cmd_x_main_buri$parseProductFrom(first_1[0],$fromShared(first_1,first_1[1]));
}
function __cmd_x_main_buri$parseSumFrom(left_0,c_1){
  while(true){
    const $t1=__cmd_x_main_buri$peek(c_1);
    if($t1[0]===1){
      const c_4=c_1;
      const $t3=__cmd_x_main_buri$parseProduct([c_4[0],c_4[1]+1n]);
      if($t3[0]!==0){
        return $t3;
      }
      const rhs_2=$t3[1];
      left_0=[1,left_0,rhs_2[0]];
      c_1=$fromShared(rhs_2,rhs_2[1]);
      continue;
    }else if($t1[0]===2){
      const c_5=c_1;
      const $t5=__cmd_x_main_buri$parseProduct([c_5[0],c_5[1]+1n]);
      if($t5[0]!==0){
        return $t5;
      }
      const rhs_3=$t5[1];
      left_0=[2,left_0,rhs_3[0]];
      c_1=$fromShared(rhs_3,rhs_3[1]);
      continue;
    }else{
      return [0,[left_0,c_1]];
    }
  }
}
function __cmd_x_main_buri$peek(c_0){
  const $t1=$list_get(c_0[0],c_0[1]);
  if($t1!==void 0){
    return $t1;
  }else if($t1===void 0){
    return $k14;
  }else{
    $abort('no arm matched');
  }
}
function __cmd_x_main_buri$parsePrimary(c_0){
  const $t1=__cmd_x_main_buri$peek(c_0);
  switch($t1[0]){
    case 0:
      {
        const c_4=$share(c_0);
        return [0,[[0,$t1[1]],[c_4[0],c_4[1]+1n]]];
      }
    case 5:
      {
        const c_5=$share(c_0);
        const $t5=__cmd_x_main_buri$parseSum([c_5[0],c_5[1]+1n]);
        if($t5[0]!==0){
          return $t5;
        }
        const inner_2=$t5[1];
        const $t6=__cmd_x_main_buri$peek(inner_2[1]);
        if($t6[0]===6){
          const c_6=$fromShared(inner_2,inner_2[1]);
          return [0,[inner_2[0],[c_6[0],c_6[1]+1n]]];
        }else{
          return $k16;
        }
      }
      break;
    case 2:
      {
        const c_7=$share(c_0);
        const $t10=__cmd_x_main_buri$parsePrimary([c_7[0],c_7[1]+1n]);
        if($t10[0]!==0){
          return $t10;
        }
        const inner_3=$t10[1];
        return [0,[[2,$k17,inner_3[0]],$fromShared(inner_3,inner_3[1])]];
      }
    default:
      {
        return $k19;
      }
  }
}
function __cmd_x_main_buri$parseProductFrom(left_0,c_1){
  while(true){
    const $t1=__cmd_x_main_buri$peek(c_1);
    if($t1[0]===3){
      const c_4=c_1;
      const $t3=__cmd_x_main_buri$parsePrimary([c_4[0],c_4[1]+1n]);
      if($t3[0]!==0){
        return $t3;
      }
      const rhs_2=$t3[1];
      left_0=[3,left_0,rhs_2[0]];
      c_1=$fromShared(rhs_2,rhs_2[1]);
      continue;
    }else if($t1[0]===4){
      const c_5=c_1;
      const $t5=__cmd_x_main_buri$parsePrimary([c_5[0],c_5[1]+1n]);
      if($t5[0]!==0){
        return $t5;
      }
      const rhs_3=$t5[1];
      left_0=[4,left_0,rhs_3[0]];
      c_1=$fromShared(rhs_3,rhs_3[1]);
      continue;
    }else{
      return [0,[left_0,c_1]];
    }
  }
}
